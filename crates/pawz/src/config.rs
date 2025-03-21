#[derive(Debug)]
pub struct AppConfig {
    pub project: &'static str,
    pub name: &'static str,
    pub version: &'static str,
    pub port: Option<u16>,
}

impl AppConfig {
    pub fn new(
        project: &'static str,
        name: &'static str,
        version: &'static str,
        port: Option<u16>,
    ) -> Self {
        AppConfig {
            project,
            name,
            version,
            port,
        }
    }

    pub fn build(project: &'static str) -> Self {
        let name = env!("CARGO_PKG_NAME");
        let version = env!("CARGO_PKG_VERSION");
        AppConfig {
            project,
            name,
            version,
            port: None,
        }
    }
}
