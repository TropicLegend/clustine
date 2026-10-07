//! A bot's connection to a server as a stream of packets.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use clustine_protocol::frame::{FrameDecoder, FrameEncoder};
use clustine_protocol::packets::{self, Packet};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// How long to wait for the server before a scenario is considered failed.
const TIMEOUT: Duration = Duration::from_secs(30);

pub struct Connection {
    stream: TcpStream,
    decoder: FrameDecoder,
    encoder: FrameEncoder,
}

impl Connection {
    /// Connects to `address` (`host:port`).
    pub async fn connect(address: &str) -> Result<Self> {
        let stream = timeout(TIMEOUT, TcpStream::connect(address))
            .await
            .context("connecting timed out")?
            .with_context(|| format!("connecting to {address}"))?;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            decoder: FrameDecoder::new(),
            encoder: FrameEncoder::new(),
        })
    }

    pub async fn write<P: Packet>(&mut self, packet: &P) -> Result<()> {
        let mut framed = Vec::new();
        self.encoder.encode(&packets::encode(packet), &mut framed)?;
        self.stream.write_all(&framed).await?;
        Ok(())
    }

    /// Waits for the next packet and returns it unframed: its id followed by its body.
    pub async fn read_frame(&mut self) -> Result<Bytes> {
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return Ok(frame);
            }
            let read = timeout(TIMEOUT, self.stream.read_buf(self.decoder.buffer()))
                .await
                .context("the server sent nothing for too long")??;
            if read == 0 {
                bail!("the server closed the connection");
            }
        }
    }
}
