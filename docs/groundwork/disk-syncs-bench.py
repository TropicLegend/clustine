"""What a sync costs on a disk, alone and together with others.

For `disk-syncs-measured.md`, section 2. It writes small files as the world store does
(a new file and fsync, an append and fdatasync, a rename and the fsync of its
directory), in turn and at the same time on threads, and prints how long each took.
Nothing of Clustine is needed to run it.

usage: python3 docs/groundwork/disk-syncs-bench.py [directory]

Without a directory it uses the default temporary directory. It removes what it wrote.
"""
import os, sys, tempfile, threading, time, statistics

base = sys.argv[1] if len(sys.argv) > 1 else None
root = tempfile.mkdtemp(prefix="clustine-syncbench-", dir=base)
payload = os.urandom(3000)


def ms(f):
    t = time.perf_counter()
    f()
    return (time.perf_counter() - t) * 1000


def new_file(path):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644)
    os.write(fd, payload)
    return fd


def sync_dir(path):
    fd = os.open(path, os.O_RDONLY)
    os.fsync(fd)
    os.close(fd)


def stats(name, times):
    times = sorted(times)
    print("%-62s median %6.1f ms   least %6.1f   most %6.1f   (n=%d)" % (
        name, statistics.median(times), times[0], times[-1], len(times)))


def together(works):
    """Runs the callables at the same time; returns how long all of them took."""
    start = threading.Barrier(len(works) + 1)
    def run(w):
        start.wait()
        w()
    threads = [threading.Thread(target=run, args=(w,)) for w in works]
    for t in threads:
        t.start()
    start.wait()
    t0 = time.perf_counter()
    for t in threads:
        t.join()
    return (time.perf_counter() - t0) * 1000


N = 30
# 1. a new file, written and synced (fsync), as a temporary is
times = []
for i in range(N):
    fd = new_file(f"{root}/new{i}")
    times.append(ms(lambda: os.fsync(fd)))
    os.close(fd)
stats("a new 3 kB file, fsync", times)

# 2. an append to an open file, fdatasync, as a commit is
log = os.open(f"{root}/log", os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
os.write(log, payload); os.fsync(log); sync_dir(root)
times = []
for i in range(N):
    os.write(log, payload)
    times.append(ms(lambda: os.fdatasync(log)))
stats("an append of 3 kB, fdatasync", times)

# 2b. an append into space that was allocated before (no change of length on disk)
pre = os.open(f"{root}/pre", os.O_WRONLY | os.O_CREAT, 0o644)
os.posix_fallocate(pre, 0, 4 * 1024 * 1024)
os.fsync(pre); sync_dir(root)
times = []
for i in range(N):
    os.pwrite(pre, payload, i * 3000)
    times.append(ms(lambda: os.fdatasync(pre)))
stats("3 kB written into allocated space, fdatasync", times)

# 3. a rename and the sync of its directory
times = []
for i in range(N):
    os.rename(f"{root}/new{i}", f"{root}/placed{i}")
    times.append(ms(lambda: sync_dir(root)))
stats("a rename, then fsync of the directory", times)

# 3b. a removal and the sync of its directory
times = []
for i in range(N):
    os.remove(f"{root}/placed{i}")
    times.append(ms(lambda: sync_dir(root)))
stats("a removal, then fsync of the directory", times)

# 4. the whole of `replace` + directory, in turn: write, fsync, rename, fsync dir
times = []
for i in range(N):
    def work():
        fd = new_file(f"{root}/state.tmp")
        os.fsync(fd); os.close(fd)
        os.rename(f"{root}/state.tmp", f"{root}/state")
        sync_dir(root)
    times.append(ms(work))
stats("write, fsync, rename, fsync directory (in turn)", times)

# 5. rounds of n new files synced together
for n in (1, 2, 3, 4, 8, 16, 32, 64):
    times = []
    for r in range(10):
        fds = [new_file(f"{root}/round{n}_{r}_{i}") for i in range(n)]
        times.append(together([(lambda fd=fd: os.fsync(fd)) for fd in fds]))
        for fd in fds:
            os.close(fd)
    stats(f"a round of {n} new files, fsync together", times)

# 6. rounds of n directories synced together, each with a file renamed in it
for n in (1, 2, 4, 8, 16):
    dirs = [f"{root}/d{n}_{i}" for i in range(n)]
    for d in dirs:
        os.mkdir(d)
    sync_dir(root)
    times = []
    for r in range(10):
        for d in dirs:
            fd = new_file(f"{d}/f{r}.tmp"); os.fsync(fd); os.close(fd)
        for d in dirs:
            os.rename(f"{d}/f{r}.tmp", f"{d}/f{r}")
        times.append(together([(lambda d=d: sync_dir(d)) for d in dirs]))
    stats(f"a round of {n} directories with a rename each, fsync together", times)

# 6b. the same bytes as one append: n times 3 kB appended to the log, one fdatasync
for n in (1, 8, 64):
    times = []
    for r in range(10):
        for i in range(n):
            os.write(log, payload)
        times.append(ms(lambda: os.fdatasync(log)))
    stats(f"{n} times 3 kB appended to one file, one fdatasync", times)

# 7. things of different kinds together: an append's fdatasync, a new file's fsync,
#    and a directory's fsync
os.mkdir(f"{root}/regions"); sync_dir(root)
times = []
for r in range(N):
    os.write(log, payload)
    fd = new_file(f"{root}/regions/tmp{r}")
    times.append(together([lambda: os.fdatasync(log), lambda: os.fsync(fd)]))
    os.close(fd)
stats("an append's fdatasync and a new file's fsync together", times)

times = []
for r in range(N):
    os.write(log, payload)
    os.rename(f"{root}/regions/tmp{r}", f"{root}/regions/placed{r}")
    times.append(together([lambda: os.fdatasync(log), lambda: sync_dir(f"{root}/regions")]))
stats("an append's fdatasync and a directory's fsync together", times)

# 8. two threads that each sync in turn, as the commit thread and the thread for
#    chunks do beside each other: how long each sync takes then
def chain(kind, out):
    for r in range(N):
        if kind == "log":
            os.write(log, payload)
            out.append(ms(lambda: os.fdatasync(log)))
        else:
            fd = new_file(f"{root}/chain{r}")
            out.append(ms(lambda: os.fsync(fd)))
            os.close(fd)
a, b = [], []
together([lambda: chain("log", a), lambda: chain("file", b)])
stats("appends' fdatasync in turn, beside new files in turn", a)
stats("new files' fsync in turn, beside appends in turn", b)

# 9. a new segment: create, append, fdatasync, then fsync of its directory (in turn),
#    against the two at the same time
os.mkdir(f"{root}/log.d"); sync_dir(root)
times = []
for r in range(N):
    def work():
        fd = os.open(f"{root}/log.d/{r}.wal", os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
        os.write(fd, payload)
        os.fdatasync(fd)
        sync_dir(f"{root}/log.d")
        os.close(fd)
    times.append(ms(work))
stats("a new segment: append, fdatasync, fsync directory (in turn)", times)
times = []
for r in range(N):
    fd = os.open(f"{root}/log.d/t{r}.wal", os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
    os.write(fd, payload)
    times.append(together([lambda: os.fdatasync(fd), lambda: sync_dir(f"{root}/log.d")]))
    os.close(fd)
stats("a new segment: fdatasync and fsync directory together", times)
times = []
for r in range(N):
    fd = os.open(f"{root}/log.d/u{r}.wal", os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
    os.write(fd, payload)
    times.append(ms(lambda: os.fsync(fd)))
    os.close(fd)
stats("a new segment: one fsync of the file alone", times)

import shutil
shutil.rmtree(root)
