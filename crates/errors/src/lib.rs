use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {}

#[cfg(test)]
mod tests {

    #[test]
    fn default_just_to_pass_checks() {}
}
