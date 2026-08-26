use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// OpenAPI to feignhttp client code generator.
#[derive(Parser)]
#[command(name = "feignhttp-generator", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate client code from an OpenAPI specification.
    Generate(GenerateArgs),
}

#[derive(Args)]
struct GenerateArgs {
    /// Path or http(s) URL of the OpenAPI spec (JSON or YAML; 2.0 / 3.0 / 3.1).
    #[arg(short, long)]
    spec: String,

    /// Output target: a directory for `--layout crate`,
    /// a file path for `--layout module`.
    #[arg(short, long)]
    out: PathBuf,

    /// Output layout.
    #[arg(long, value_enum, default_value_t = LayoutArg::Module)]
    layout: LayoutArg,

    /// Package name for `--layout crate`.
    #[arg(long, default_value = "generated-api")]
    package_name: String,

    /// Local path dependency used for feignhttp in generated Cargo.toml.
    #[arg(long)]
    feignhttp_path: Option<String>,
}

#[derive(ValueEnum, Clone)]
enum LayoutArg {
    Crate,
    Module,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Generate(args) => {
            let options = feignhttp_generator::Options {
                package_name: args.package_name.clone(),
                layout: match args.layout {
                    LayoutArg::Crate => feignhttp_generator::Layout::Crate,
                    LayoutArg::Module => feignhttp_generator::Layout::Module,
                },
                feignhttp_dep: match &args.feignhttp_path {
                    Some(p) => feignhttp_generator::FeignDep::Path(p.clone()),
                    None => feignhttp_generator::FeignDep::default(),
                },
            };
            feignhttp_generator::generate_from_source(&args.spec, &args.out, &options)?;
            println!("generated {} -> {}", args.spec, args.out.display());
            Ok(())
        }
    }
}
