use nexus_macros::Command;

fn main() {
    #[allow(dead_code)]
    #[derive(Command)]
    #[command(result = User, error = CreateUserError)]
    struct CreateUser {}
}
