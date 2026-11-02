fn main() {
    jotto::run_cli(jotto::game::GameMode::Wordle, std::env::args_os());
}
