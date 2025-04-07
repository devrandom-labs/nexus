#[derive(Debug)]
pub struct HashedPassword(String);

impl HashedPassword {
    pub fn new(hash: String) -> Self {
        Self(hash)
    }
}
