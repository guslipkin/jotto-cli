fn main() {
    jotto::run_cli(jotto::game::GameMode::Jotto, std::env::args_os());
}
