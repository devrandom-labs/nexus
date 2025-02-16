#![allow(dead_code)]

pub trait MessageHandler {
    type Response;
    type Error;
    fn execute(self) -> Result<Self::Response, Self::Error>;
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn basic_message_handler_test() {
        struct Test(String);

        impl MessageHandler for Test {
            type Response = ();
            type Error = String;
            fn execute(self) -> Result<Self::Response, Self::Error> {
                Ok(())
            }
        }
        let test_message = Test("hello".to_string());
        let reply = test_message.execute();
        assert!(reply.is_ok());
        let reply = reply.unwrap();
        assert_eq!(reply, ());
    }
}
