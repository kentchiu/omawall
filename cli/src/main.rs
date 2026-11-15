mod app;
mod cache;
mod color;
mod config;
mod error;
mod image;
mod pages;
mod paths;
mod select;
mod state;
mod sync;
#[cfg(test)]
mod testutil;
mod wallhaven;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args == ["--version"] {
        println!("omawall {}", app::VERSION);
        return;
    }
    let result = app::parse_args(&args)
        .and_then(|command| Ok((command, paths::Paths::from_env()?)))
        .and_then(|(command, paths)| app::App::new(paths).run(command));
    let (out, code) = app::render(result);
    println!("{out}");
    std::process::exit(code);
}
