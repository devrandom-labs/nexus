//! The `EventStream` trait declares `next()` returns `impl Future<...> + Send`
//! (see `crates/nexus-store/src/stream/cursor.rs`). A stream whose internal
//! state is `!Send` (e.g. contains `Rc<T>`) cannot produce a `Send` future
//! from `&mut self`, so it cannot implement the trait. This is the structural
//! Send guarantee — there is no way to construct a `!Send` `EventStream`,
//! which transitively means combinator wrappers (`Map`, `TryMap`, `TryScan`)
//! are always `Send` when their closure / state are.

use std::convert::Infallible;
use std::rc::Rc;

use nexus_store::envelope::PersistedEnvelope;
use nexus_store::stream::EventStream;

struct NotSendStream(Rc<()>);

impl EventStream for NotSendStream {
    type Item<'a> = PersistedEnvelope;
    type Error = Infallible;

    async fn next(&mut self) -> Result<Option<PersistedEnvelope>, Self::Error> {
        let _ = &self.0;
        Ok(None)
    }
}

fn main() {}
