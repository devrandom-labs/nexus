//! `EventStream::next()` returns an envelope that borrows from `&mut self`.
//! Holding that envelope and calling `next()` again would alias the cursor,
//! so the borrow checker must reject it. This proves the lending-iterator
//! GAT contract is structurally enforced — no runtime checks needed.

use std::convert::Infallible;
use std::future::Future;

use nexus::Version;
use nexus_store::envelope::PersistedEnvelope;
use nexus_store::stream::EventStream;

struct Single {
    served: bool,
    payload: Vec<u8>,
}

impl EventStream for Single {
    type Error = Infallible;

    fn next(
        &mut self,
    ) -> impl Future<Output = Result<Option<PersistedEnvelope<'_>>, Self::Error>> + Send {
        async move {
            if self.served {
                return Ok(None);
            }
            self.served = true;
            Ok(Some(PersistedEnvelope::new_unchecked(
                Version::new(1).unwrap(),
                "E",
                1,
                &self.payload,
                (),
            )))
        }
    }
}

#[tokio::main]
async fn main() {
    let mut stream = Single {
        served: false,
        payload: vec![1, 2, 3],
    };

    let env = stream.next().await.unwrap().unwrap();
    // Holding `env` here aliases the cursor — calling `next()` again must fail.
    let _again = stream.next().await;
    // Use `env` after the second `next()` to force the borrow to overlap.
    let _ = env.payload();
}
