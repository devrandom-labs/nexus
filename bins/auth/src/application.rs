#[derive(Debug, PartialEq, Eq)]
pub enum Env {
    DEV,
    PROD,
    LOCAL,
}

impl Default for Env {
    fn default() -> Self {
        Self::LOCAL
    }
}

#[derive(Default, Debug)]
pub struct Application {
    env: Env,
}
