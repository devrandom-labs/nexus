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
pub struct Application<'a> {
    project: &'a str,
    name: &'a str,
    version: &'a str,
    env: Env,
}

impl Application<'a> {
    pub fn new(project: &'a str, name: &'a str, version: &'a str) -> Self {
        Application {
            project,
            name,
            version,
            env,
        }
    }
}
