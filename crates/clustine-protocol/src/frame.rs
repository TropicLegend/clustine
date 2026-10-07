//! Splitting a byte stream into packets and back, with optional zlib compression.
//!
//! On the wire every packet is prefixed with its length as a VarInt. Once compression
//! has been enabled for a connection, the framed content starts with a second VarInt:
//! the uncompressed size, or 0 if the packet was sent uncompressed because it is smaller
//! than the threshold.
//!
//! Nothing here performs I/O. The caller feeds received bytes into a [`FrameDecoder`]
//! and sends what a [`FrameEncoder`] produces.

use std::io::{Read, Write};

use bytes::{Buf, Bytes, BytesMut};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

use crate::codec::{Reader, Writer, var_int_len};

/// The largest framed length: the length prefix is at most three bytes long.
pub const MAX_FRAME_LENGTH: usize = (1 << 21) - 1;

/// The largest packet accepted after decompression, as in vanilla.
pub const MAX_UNCOMPRESSED_LENGTH: usize = 1 << 23;

/// Why a byte stream could not be framed. The connection cannot recover from any of these.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("length prefix is longer than 3 bytes")]
    LengthPrefixTooLong,
    #[error("packet of {0} bytes is too large to frame")]
    TooLarge(usize),
    #[error("malformed compression header")]
    BadCompressionHeader,
    #[error("uncompressed size {0} is larger than the protocol maximum")]
    UncompressedTooLarge(usize),
    #[error("compressed data is corrupt or does not match its declared size")]
    BadCompressedData,
}

/// Turns received bytes into packets.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: BytesMut,
    compression: bool,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Expects the compressed packet format from the next packet on.
    pub fn enable_compression(&mut self) {
        self.compression = true;
    }

    /// The buffer to append received bytes to, for example with a socket read.
    pub fn buffer(&mut self) -> &mut BytesMut {
        &mut self.buffer
    }

    /// Appends received bytes.
    pub fn queue(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Returns the next complete packet (its id and body), or `None` if more bytes are needed.
    pub fn next_frame(&mut self) -> Result<Option<Bytes>, FrameError> {
        let mut length = 0;
        let mut prefix_length = 0;
        loop {
            let Some(&byte) = self.buffer.get(prefix_length) else {
                return Ok(None);
            };
            length |= usize::from(byte & 0x7F) << (7 * prefix_length);
            prefix_length += 1;
            if byte & 0x80 == 0 {
                break;
            }
            if prefix_length == 3 {
                return Err(FrameError::LengthPrefixTooLong);
            }
        }
        if self.buffer.len() < prefix_length + length {
            return Ok(None);
        }
        self.buffer.advance(prefix_length);
        let frame = self.buffer.split_to(length).freeze();
        if !self.compression {
            return Ok(Some(frame));
        }

        let mut reader = Reader::new(&frame);
        let declared = reader
            .var_int()
            .map_err(|_| FrameError::BadCompressionHeader)?;
        let header_length = frame.len() - reader.remaining();
        let body = frame.slice(header_length..);
        match usize::try_from(declared) {
            Err(_) => Err(FrameError::BadCompressionHeader),
            Ok(0) => Ok(Some(body)),
            Ok(size) if size > MAX_UNCOMPRESSED_LENGTH => {
                Err(FrameError::UncompressedTooLarge(size))
            }
            Ok(size) => inflate(&body, size).map(|packet| Some(packet.into())),
        }
    }
}

/// Decompresses `data`, which must inflate to exactly `size` bytes.
fn inflate(data: &[u8], size: usize) -> Result<Vec<u8>, FrameError> {
    let mut packet = Vec::with_capacity(size);
    // Reading one byte past the declared size detects data that inflates to more.
    ZlibDecoder::new(data)
        .take(size as u64 + 1)
        .read_to_end(&mut packet)
        .map_err(|_| FrameError::BadCompressedData)?;
    if packet.len() != size {
        return Err(FrameError::BadCompressedData);
    }
    Ok(packet)
}

/// Turns packets into bytes to send.
#[derive(Debug, Default)]
pub struct FrameEncoder {
    threshold: Option<usize>,
}

impl FrameEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Uses the compressed packet format from the next packet on, compressing packets of
    /// at least `threshold` bytes.
    pub fn enable_compression(&mut self, threshold: usize) {
        self.threshold = Some(threshold);
    }

    /// Appends the framed form of `packet` (its id and body) to `out`.
    pub fn encode(&self, packet: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
        let Some(threshold) = self.threshold else {
            return put_frame(&[], packet, out);
        };
        // A declared size of 0 means "not compressed", so an empty packet cannot be sent
        // compressed even if the threshold is 0.
        if packet.len() < threshold || packet.is_empty() {
            return put_frame(&[0], packet, out);
        }
        if packet.len() > MAX_UNCOMPRESSED_LENGTH {
            return Err(FrameError::TooLarge(packet.len()));
        }

        let mut header = Writer::new();
        header.put_length(packet.len());
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        // Writing into a `Vec` cannot fail.
        encoder.write_all(packet).expect("compressing into memory");
        let compressed = encoder.finish().expect("compressing into memory");
        put_frame(header.as_bytes(), &compressed, out)
    }
}

fn put_frame(header: &[u8], body: &[u8], out: &mut Vec<u8>) -> Result<(), FrameError> {
    let length = header.len() + body.len();
    if length > MAX_FRAME_LENGTH {
        return Err(FrameError::TooLarge(length));
    }
    let mut prefix = Writer::new();
    prefix.put_length(length);
    out.reserve(var_int_len(length as i32) + length);
    out.extend_from_slice(prefix.as_bytes());
    out.extend_from_slice(header);
    out.extend_from_slice(body);
    Ok(())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn decode_all(decoder: &mut FrameDecoder) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        while let Some(frame) = decoder.next_frame().unwrap() {
            frames.push(frame.to_vec());
        }
        frames
    }

    #[test]
    fn uncompressed_known_answer() {
        let mut out = Vec::new();
        FrameEncoder::new().encode(&[0x00, 0xAB], &mut out).unwrap();
        assert_eq!(out, [0x02, 0x00, 0xAB]);
    }

    #[test]
    fn small_packet_below_threshold_known_answer() {
        let mut encoder = FrameEncoder::new();
        encoder.enable_compression(256);
        let mut out = Vec::new();
        encoder.encode(&[0x00, 0xAB], &mut out).unwrap();
        // Length 3, then "not compressed", then the packet.
        assert_eq!(out, [0x03, 0x00, 0x00, 0xAB]);
    }

    #[test]
    fn large_packet_is_compressed() {
        let packet = vec![7; 10_000];
        let mut encoder = FrameEncoder::new();
        encoder.enable_compression(256);
        let mut out = Vec::new();
        encoder.encode(&packet, &mut out).unwrap();
        assert!(out.len() < 200, "10,000 equal bytes compress well");

        let mut decoder = FrameDecoder::new();
        decoder.enable_compression();
        decoder.queue(&out);
        assert_eq!(decode_all(&mut decoder), [packet]);
    }

    /// Found by the round-trip property test: with a threshold of 0 an empty packet used
    /// to be compressed with a declared size of 0, which reads as "not compressed".
    #[test]
    fn empty_packet_is_never_compressed() {
        let mut encoder = FrameEncoder::new();
        encoder.enable_compression(0);
        let mut out = Vec::new();
        encoder.encode(&[], &mut out).unwrap();
        assert_eq!(out, [0x01, 0x00]);

        let mut decoder = FrameDecoder::new();
        decoder.enable_compression();
        decoder.queue(&out);
        assert_eq!(decode_all(&mut decoder), [Vec::<u8>::new()]);
    }

    #[test]
    fn incomplete_input_yields_nothing() {
        let mut decoder = FrameDecoder::new();
        assert_eq!(decoder.next_frame(), Ok(None));
        decoder.queue(&[0x80]);
        assert_eq!(decoder.next_frame(), Ok(None));
        decoder.queue(&[0x01]);
        assert_eq!(decoder.next_frame(), Ok(None));
        decoder.queue(&[0; 127]);
        assert_eq!(decoder.next_frame(), Ok(None));
        decoder.queue(&[0]);
        assert_eq!(decoder.next_frame().unwrap().unwrap().len(), 128);
    }

    #[test]
    fn four_byte_length_prefix_is_rejected() {
        let mut decoder = FrameDecoder::new();
        decoder.queue(&[0x80, 0x80, 0x80, 0x01]);
        assert_eq!(decoder.next_frame(), Err(FrameError::LengthPrefixTooLong));
    }

    #[test]
    fn oversized_packet_is_not_framed() {
        let packet = vec![0; MAX_FRAME_LENGTH + 1];
        let mut out = Vec::new();
        assert_eq!(
            FrameEncoder::new().encode(&packet, &mut out),
            Err(FrameError::TooLarge(MAX_FRAME_LENGTH + 1))
        );
        assert!(out.is_empty());
    }

    #[test]
    fn declared_size_must_match() {
        let mut encoder = FrameEncoder::new();
        encoder.enable_compression(0);
        let mut framed = Vec::new();
        encoder.encode(&[1; 100], &mut framed).unwrap();
        // The size follows the one-byte length prefix. 100 becomes 99, then 101.
        for wrong in [99, 101] {
            let mut tampered = framed.clone();
            tampered[1] = wrong;
            let mut decoder = FrameDecoder::new();
            decoder.enable_compression();
            decoder.queue(&tampered);
            assert_eq!(decoder.next_frame(), Err(FrameError::BadCompressedData));
        }
    }

    #[test]
    fn huge_declared_size_is_rejected_without_inflating() {
        let mut content = Writer::new();
        content.put_length(MAX_UNCOMPRESSED_LENGTH + 1);
        let mut framed = Vec::new();
        put_frame(content.as_bytes(), &[], &mut framed).unwrap();
        let mut decoder = FrameDecoder::new();
        decoder.enable_compression();
        decoder.queue(&framed);
        assert_eq!(
            decoder.next_frame(),
            Err(FrameError::UncompressedTooLarge(
                MAX_UNCOMPRESSED_LENGTH + 1
            ))
        );
    }

    fn packets_strategy() -> impl Strategy<Value = Vec<Vec<u8>>> {
        // Runs of equal bytes make some packets long and compressible.
        let packet = prop::collection::vec((any::<u8>(), 1usize..300), 0..20).prop_map(|runs| {
            runs.into_iter()
                .flat_map(|(byte, count)| std::iter::repeat_n(byte, count))
                .collect::<Vec<u8>>()
        });
        prop::collection::vec(packet, 0..8)
    }

    proptest! {
        /// Packets survive framing whatever the threshold and however the bytes arrive.
        #[test]
        fn frames_round_trip(
            packets in packets_strategy(),
            threshold in prop::option::of(0usize..600),
            chunk_size in 1usize..500,
        ) {
            let mut encoder = FrameEncoder::new();
            let mut decoder = FrameDecoder::new();
            if let Some(threshold) = threshold {
                encoder.enable_compression(threshold);
                decoder.enable_compression();
            }
            let mut stream = Vec::new();
            for packet in &packets {
                encoder.encode(packet, &mut stream).unwrap();
            }

            let mut received = Vec::new();
            for chunk in stream.chunks(chunk_size) {
                decoder.queue(chunk);
                received.extend(decode_all(&mut decoder));
            }
            prop_assert_eq!(received, packets);
        }

        /// The decoder returns instead of panicking, whatever the input.
        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>, compression: bool) {
            let mut decoder = FrameDecoder::new();
            if compression {
                decoder.enable_compression();
            }
            decoder.queue(&bytes);
            while let Ok(Some(_)) = decoder.next_frame() {}
        }
    }
}
