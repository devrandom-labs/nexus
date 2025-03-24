use ulid::Ulid;

pub struct UserId(Ulid);

#[derive(Debug)]
pub struct Email {
    inner: String,
}

#[derive(Debug)]
pub struct Password {
    inner: String,
}

pub struct User {
    user_id: UserId,
    email: Email,
    password: Password,
}
