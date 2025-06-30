use nexus_macros::Command;

fn main() {
    #[allow(dead_code)]
    #[derive(Command)]
    #[command(id = String, result = CreateUserError)]
    struct CreateUser {}
}
