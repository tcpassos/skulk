//! Async client for the implant control protocol. Speaks the same JSON-lines
//! wire format as any controller and depends only on `contract`. This is the
//! reference implementation of "how to drive the engine over a socket" — the CLI,
//! a TUI, and integration tests all build on it.

use std::io;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;

use contract::{
    Body, Command, Envelope, LootEntry, LootQuery, Manifest, MessageId, ProtocolError, TaskResult,
};

/// The terminal outcome of a command.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The command completed with a result.
    Result(TaskResult),
    /// The command was rejected (unknown module, missing capability, ...).
    Error(ProtocolError),
}

/// A connection to one implant.
pub struct Client {
    reader: tokio::io::Lines<BufReader<OwnedReadHalf>>,
    writer: OwnedWriteHalf,
}

impl Client {
    /// Connect to an implant at `addr`, e.g. `"127.0.0.1:9000"`.
    pub async fn connect(addr: &str) -> io::Result<Client> {
        let stream = TcpStream::connect(addr).await?;
        let (read, writer) = stream.into_split();
        Ok(Client { reader: BufReader::new(read).lines(), writer })
    }

    /// Send a command; returns the message id, for correlating the responses.
    pub async fn send(&mut self, command: Command) -> io::Result<MessageId> {
        let env = Envelope::new(Body::Command(command), 0);
        let id = env.id;
        let mut buf = serde_json::to_vec(&env).map_err(to_io)?;
        buf.push(b'\n');
        self.writer.write_all(&buf).await?;
        self.writer.flush().await?;
        Ok(id)
    }

    /// Read the next envelope from the stream, or `None` if the peer closed.
    pub async fn recv(&mut self) -> io::Result<Option<Envelope>> {
        loop {
            match self.reader.next_line().await? {
                None => return Ok(None),
                Some(line) if line.trim().is_empty() => continue,
                Some(line) => return serde_json::from_str(&line).map(Some).map_err(to_io),
            }
        }
    }

    /// Send a command and drive it to completion, calling `on_event` for each
    /// intermediate envelope (Ack, progress, logs, alerts, loot events) correlated
    /// to it, and returning the terminal Result/Error.
    pub async fn run(
        &mut self,
        command: Command,
        mut on_event: impl FnMut(&Envelope),
    ) -> io::Result<Outcome> {
        let id = self.send(command).await?;
        loop {
            let env = self
                .recv()
                .await?
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed"))?;
            if env.correlate != Some(id) {
                continue; // unsolicited (e.g. heartbeat) — ignore for a one-shot command
            }
            match &env.body {
                Body::Result(r) => return Ok(Outcome::Result(r.clone())),
                Body::Error(e) => return Ok(Outcome::Error(e.clone())),
                _ => on_event(&env),
            }
        }
    }

    /// Stream every inbound envelope (including unsolicited events) until the peer
    /// closes, calling `on_event` for each.
    pub async fn watch(&mut self, mut on_event: impl FnMut(&Envelope)) -> io::Result<()> {
        while let Some(env) = self.recv().await? {
            on_event(&env);
        }
        Ok(())
    }

    /// Fetch the implant's manifest (its modules and capabilities).
    pub async fn describe(&mut self) -> io::Result<Manifest> {
        match self.run(Command::Describe, |_| {}).await? {
            Outcome::Result(r) => serde_json::from_value(r.output.0).map_err(to_io),
            Outcome::Error(e) => Err(rejected("describe", &e)),
        }
    }

    /// Query stored loot.
    pub async fn loot(&mut self, query: LootQuery) -> io::Result<Vec<LootEntry>> {
        match self.run(Command::Loot(query), |_| {}).await? {
            Outcome::Result(r) => serde_json::from_value(r.output.0).map_err(to_io),
            Outcome::Error(e) => Err(rejected("loot", &e)),
        }
    }
}

impl Client {
    /// Split into an independent [`Sender`] and [`Receiver`] over the one
    /// connection, for a UI that sends commands while continuously consuming the
    /// event stream.
    pub fn split(self) -> (Sender, Receiver) {
        (Sender { writer: self.writer }, Receiver { reader: self.reader })
    }
}

/// The write half of a split [`Client`].
pub struct Sender {
    writer: OwnedWriteHalf,
}

/// The read half of a split [`Client`].
pub struct Receiver {
    reader: tokio::io::Lines<BufReader<OwnedReadHalf>>,
}

impl Sender {
    /// Send a command; returns the message id for correlating the responses.
    pub async fn send(&mut self, command: Command) -> io::Result<MessageId> {
        let env = Envelope::new(Body::Command(command), 0);
        let id = env.id;
        let mut buf = serde_json::to_vec(&env).map_err(to_io)?;
        buf.push(b'\n');
        self.writer.write_all(&buf).await?;
        self.writer.flush().await?;
        Ok(id)
    }
}

impl Receiver {
    /// Read the next envelope, or `None` if the peer closed.
    pub async fn recv(&mut self) -> io::Result<Option<Envelope>> {
        loop {
            match self.reader.next_line().await? {
                None => return Ok(None),
                Some(line) if line.trim().is_empty() => continue,
                Some(line) => return serde_json::from_str(&line).map(Some).map_err(to_io),
            }
        }
    }
}

fn to_io<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

fn rejected(what: &str, e: &ProtocolError) -> io::Error {
    io::Error::new(io::ErrorKind::Other, format!("{what} rejected: {}", e.message))
}
