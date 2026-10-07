# Deployment

Clustine as separate services on Kubernetes, and a test that runs them on a local
cluster and lets bots walk from one worker to another.

| Path | Contents |
|---|---|
| [`../Dockerfile`](../Dockerfile) | The image of every service: the `clustine` binary, whose subcommand says which service a container is, and the test client `clustine-botswarm` |
| [`kubernetes/`](kubernetes) | A kustomize base in the namespace `clustine`: one coordinator, one world store with a volume, two workers and one edge |
| [`kubernetes/test/bots.yaml`](kubernetes/test/bots.yaml) | A Job that lets bots walk across the boundary between the two workers |
| [`kind/`](kind) | The scripts that run all of this on a cluster made with [kind](https://kind.sigs.k8s.io), which runs Kubernetes in Docker containers |

## Keep it inside the cluster

The server runs in offline mode: it does not check who a client is and believes any
name it is given, so whoever reaches the edge can play as anybody. The links between the
services are plain TCP without authentication as well. Every Service here is therefore
only reachable inside the cluster, and it has to stay that way: do not add a
LoadBalancer, a NodePort or an Ingress. On a cluster that you share, keep in mind that
its other pods can reach the services too.

## The cluster test

It needs Docker, `kubectl` and kind. If kind is not installed,
this downloads a fixed release of it to `target/tools/kind` and checks its checksum:

```bash
deploy/kind/get-kind.sh
```

The test itself:

```bash
deploy/kind/test.sh
```

It builds the image `clustine:dev`, creates the cluster `clustine-test`, loads the image
into it, applies `kubernetes/`, waits until the services are ready and runs the bots.
The world is divided at block x = 64, each worker runs one side, and the bots walk back
and forth between x = 40.5 and x = 90.5. The test passes if no bot was disconnected, a
watching bot saw each walker as exactly one entity the whole time, and each of the two
workers logged players arriving from and departing to another region. If it fails, it
prints the end of every pod's log, the events and the list of pods. The cluster is
deleted at the end either way.

| Setting | Effect |
|---|---|
| `KEEP=1` | Leave the cluster running at the end |
| `--reuse` | Use the cluster of an earlier run instead of replacing it; what was deployed on it is removed first |
| `SKIP_BUILD=1` | Do not build the image if `clustine:dev` exists already |
| `KIND=/path/to/kind` | The kind to use instead of the one in `PATH` or in `target/tools` |
| `ROLLOUT_TIMEOUT`, `BOTS_TIMEOUT` | Seconds to wait for each service and for the bots (180 and 300) |

The test keeps the cluster's kubeconfig in `target/kind/kubeconfig` and leaves
`~/.kube/config` and its current context alone.

[`.github/workflows/cluster.yml`](../.github/workflows/cluster.yml) runs the same script
on pushes to `main` that change the image or the deployment.

## Playing on the cluster

Leave the cluster running after the test and forward the edge's port to your machine,
from the repository root:

```bash
KEEP=1 deploy/kind/test.sh
```

```bash
kubectl --kubeconfig target/kind/kubeconfig --namespace clustine port-forward service/clustine-edge 25565:25565
```

Then connect a Minecraft: Java Edition 26.3 client to `localhost:25565`. Walking east
past x = 64 takes you from one worker to the other, which shows in the workers' logs:

```bash
kubectl --kubeconfig target/kind/kubeconfig --namespace clustine logs --follow --prefix --selector app.kubernetes.io/name=clustine-worker
```

`kubectl port-forward` listens on localhost only. Do not give it another `--address`,
for the reason above.

Delete the cluster when you are done, with `target/tools/kind` in place of `kind` if
that is where it is:

```bash
kind delete cluster --name clustine-test
```

## Another cluster

The manifests name the image `clustine:dev`, which exists in no registry and suits a
cluster that the image is loaded into. For one that pulls from a registry, push the
image there and name it in a kustomize overlay on top of `kubernetes/`. The world store
asks for a volume of 1 Gi from the cluster's default storage class.
