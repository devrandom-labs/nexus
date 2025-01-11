pub trait Event {
    fn get_time_stamp(&self) -> &str;
}
