//! `DecoderBuilder::build()` is only callable on the `WithCodec` or
//! `WithBorrowingCodec` typestate. Calling `.build()` directly after
//! `.decoder()` (which yields `NeedsCodec`) must fail to compile —
//! that's what protects users from constructing a `DecodedStream` with
//! no codec wired up.

use std::convert::Infallible;
use std::future::Future;

use nexus_store::envelope::PersistedEnvelope;
use nexus_store::stream::{EventStream, EventStreamExt};

struct Empty;

impl EventStream for Empty {
    type Error = Infallible;

    fn next(
        &mut self,
    ) -> impl Future<Output = Result<Option<PersistedEnvelope<'_>>, Self::Error>> + Send {
        async { Ok(None) }
    }
}

fn main() {
    let stream = Empty;
    // `.build()` is not available on `DecoderBuilder<_, NeedsCodec, NoUpcaster>`.
    let _: () = stream.decoder().build::<()>();
}
