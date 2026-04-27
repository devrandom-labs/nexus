use std::pin::Pin;
use std::task::{Context, Poll};

use nexus::{DomainEvent, Version};
use nexus_store::Codec;
use nexus_store::store::EventStream;
use tokio_stream::Stream;

/// Adapter that converts a lending GAT [`EventStream`] into an owned
/// [`tokio_stream::Stream`] by decoding events while the lending borrow
/// is alive.
///
/// Each call to `poll_next` polls the underlying `EventStream::next()`,
/// decodes the event from the borrowed envelope, and yields the owned
/// `(Version, E)` pair. The lending borrow is released before the item
/// is returned.
pub(crate) struct DecodedStream<'a, S, EC, E> {
    stream: S,
    codec: &'a EC,
    _event: std::marker::PhantomData<E>,
}

impl<'a, S, EC, E> DecodedStream<'a, S, EC, E> {
    pub(crate) fn new(stream: S, codec: &'a EC) -> Self {
        Self {
            stream,
            codec,
            _event: std::marker::PhantomData,
        }
    }
}

// SAFETY: DecodedStream never structurally pins any of its fields.
// We only access them via &mut self (from Pin::get_mut), and
// EventStream::next() takes &mut self, not Pin<&mut Self>.
impl<S, EC, E> Unpin for DecodedStream<'_, S, EC, E> {}

/// Error from the decoded stream — either a subscription/stream error or a codec error.
#[derive(Debug)]
pub(crate) enum DecodeStreamError<SubErr, CodecErr> {
    Stream(SubErr),
    Codec(CodecErr),
}

impl<S, EC, E> Stream for DecodedStream<'_, S, EC, E>
where
    S: EventStream<()>,
    EC: Codec<E>,
    E: DomainEvent,
{
    type Item = Result<(Version, E), DecodeStreamError<S::Error, EC::Error>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        // Pin the next() future on the stack, poll it, decode while borrow is alive.
        let mut next_fut = std::pin::pin!(this.stream.next());
        match next_fut.as_mut().poll(cx) {
            Poll::Ready(Ok(Some(envelope))) => {
                let version = envelope.version();
                let result = this
                    .codec
                    .decode(envelope.event_type(), envelope.payload())
                    .map(|event| (version, event))
                    .map_err(DecodeStreamError::Codec);
                Poll::Ready(Some(result))
            }
            Poll::Ready(Ok(None)) => Poll::Ready(None),
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(DecodeStreamError::Stream(e)))),
            Poll::Pending => Poll::Pending,
        }
    }
}
