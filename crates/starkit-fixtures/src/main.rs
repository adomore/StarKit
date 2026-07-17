//! starkit-fixtures CLI (task T0-1).
//!
//! ```text
//! starkit-fixtures gen --suite basic-5k --out fixtures/generated
//! starkit-fixtures gen --suite all      --out fixtures/generated
//! ```
//!
//! `--seed` is optional and defaults to the suite's canonical seed (D-007); it
//! may only be given for a single suite, since `all` means "the five committed
//! suites at their canonical seeds" and an override would quietly produce
//! artifacts that do not match the committed manifest.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use starkit_fixtures::{emit, generate_suite, SUITES};

#[derive(Parser)]
#[command(
    name = "starkit-fixtures",
    about = "Synthetic golden star-field generator (docs/FIXTURES.md)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate one suite (or `all`) into an output directory.
    Gen {
        /// Suite name, or `all` for the five v1 suites.
        #[arg(long)]
        suite: String,
        /// Override the canonical seed. Single suite only.
        #[arg(long)]
        seed: Option<u64>,
        /// Output root; each suite lands in `<out>/<suite>/`.
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Gen { suite, seed, out } => {
            let suites: Vec<&str> = if suite == "all" {
                if seed.is_some() {
                    bail!(
                        "--seed cannot be combined with --suite all: the committed manifest \
                         pins each suite to its canonical seed (D-007). Generate a single \
                         suite to experiment with a different seed."
                    );
                }
                SUITES.to_vec()
            } else {
                vec![suite.as_str()]
            };

            let mut lines = Vec::new();
            for s in &suites {
                let g = generate_suite(s, seed, &out)
                    .with_context(|| format!("generating suite '{s}'"))?;
                println!(
                    "{s}: {} artifacts, seed {}",
                    g.manifest_lines.len(),
                    g.params.seed
                );
                lines.extend(g.manifest_lines);
            }

            let body = emit::render_manifest(&mut lines);
            let path = out.join("MANIFEST.sha256");
            std::fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;
            println!("wrote {}", path.display());
        }
    }
    Ok(())
}
