use serde::{Deserialize, Serialize};

use crate::{Ack, Command, Event, MessageId, ProtocolError, TaskResult};

/// The single message type on the wire and on the internal bus.
///
/// In the "dumb daemon" design the socket message and the bus event are the same
/// object; this is that object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// Protocol version (see [`crate::PROTOCOL_VERSION`]).
    pub v: u16,
    /// Unique id of this message.
    pub id: MessageId,
    /// The message this one answers or relates to, if any. This is what lets an
    /// async controller match a response/event back to the command that caused
    /// it, with many commands in flight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlate: Option<MessageId>,
    /// Milliseconds since the Unix epoch, stamped by the sender.
    pub ts: u64,
    pub body: Body,
}

impl Envelope {
    /// New message stamped with the current protocol version and a fresh id.
    pub fn new(body: Body, ts: u64) -> Self {
        Self {
            v: crate::PROTOCOL_VERSION,
            id: MessageId::new(),
            correlate: None,
            ts,
            body,
        }
    }

    /// Mark this message as a reply to `cause`.
    pub fn in_reply_to(mut self, cause: MessageId) -> Self {
        self.correlate = Some(cause);
        self
    }
}

/// Direction and kind of an [`Envelope`]. Adjacently tagged so the concrete
/// payload types stay clean: `{ "kind": "command", "data": { .. } }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Body {
    /// Controller -> implant.
    Command(Command),
    /// Implant -> controller: command accepted, task started.
    Ack(Ack),
    /// Implant -> controller: terminal outcome of a task.
    Result(TaskResult),
    /// Implant -> controller: unsolicited async event.
    Event(Event),
    /// Implant -> controller: protocol or dispatch error.
    Error(ProtocolError),
}
