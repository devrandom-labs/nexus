use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(u64);

impl Version {
    pub const INITIAL: Version = Version(0);

    pub fn next(self) -> Version {
        Version(self.0 + 1)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for Version {
    fn from(v: u64) -> Self {
        Version(v)
    }
}

#[derive(Debug)]
pub struct VersionedEvent<E> {
    pub version: Version,
    pub event: E,
}
