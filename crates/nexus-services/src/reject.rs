pub struct Rejection {
    reason: Reason,
}

pub enum Reason {
    NotFound,
    Other(Box<Rejections>),
}

pub enum Rejections {}
