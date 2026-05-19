//! The `EventStream` trait declares `next()` returns `impl Future<...> + Send`
//! (see `crates/nexus-store/src/stream.rs` line 41). A stream whose internal
//! state is `!Send` (e.g. contains `Rc<T>`) cannot produce a `Send` future
//! from `&mut self`, so it cannot implement the trait. This is the structural
//! Send guarantee — there is no way to construct a `!Send` `EventStream`,
//! which transitively means `DecodedStream`/`BorrowedDecodedStream` are
//! always `Send` when their codec and upcaster are.

use std::convert::Infallible;
use std::rc::Rc;

use nexus_store::envelope::PersistedEnvelope;
use nexus_store::stream::EventStream;

struct NotSendStream(Rc<()>);

impl EventStream for NotSendStream {
    type Error = Infallible;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope<'_>>, Self::Error> {
        let _ = &self.0;
        Ok(None)
    }
}

fn main() {}
