fn main() {
    flatbed_build::Config::new()
        .schema("schemas/sentry.fbs")
        .compile()
        .expect("flatbed codegen failed");
}
