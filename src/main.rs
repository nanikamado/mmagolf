use clap::{Parser, Subcommand};
use json::object;
use std::{fs::read_to_string, io::Read, process::exit};

#[derive(Debug, Parser)]
#[clap(version, about, long_about = None)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Submit code
    #[clap(arg_required_else_help = true)]
    Submit {
        #[clap(short, long, value_name = "FILE")]
        code: String,
        #[clap(short, long)]
        lang: String,
        #[clap(short, long)]
        problem_number: usize,
    },
    /// Run the code in the judge surver to see if the code works
    #[clap(
        arg_required_else_help = true,
        override_usage = "echo <INPUT> | mmagolf codetest --code <CODE> --lang <LANG>"
    )]
    Codetest {
        /// Source code
        #[clap(short, long, value_name = "FILE")]
        code: String,
        /// Language
        #[clap(short, long)]
        lang: String,
    },
}

fn main() {
    let args = Cli::parse();

    match args.command {
        Commands::Submit {
            code,
            lang,
            problem_number,
        } => {
            let code = read_to_string(&mut code.clone()).unwrap_or_else(|e| {
                eprintln!("{}: {}", code, e);
                exit(1)
            });
            println!(
                "{}",
                object! {
                    "type": "submit",
                    "lang": lang,
                    "problem-number": problem_number,
                    "code": code,
                }
            )
        }
        Commands::Codetest { code, lang } => {
            let code = read_to_string(&mut code.clone()).unwrap_or_else(|e| {
                eprintln!("{}: {}", code, e);
                exit(1)
            });
            let input = if atty::is(atty::Stream::Stdin) {
                None
            } else {
                Some({
                    let mut input = Vec::new();
                    std::io::stdin()
                        .read_to_end(&mut input)
                        .unwrap_or_else(|e| {
                            eprintln!("{e}");
                            exit(1)
                        });
                    input
                })
            };
            println!(
                "{}",
                object! {
                    "type": "codetest",
                    "lang": lang,
                    "code": code,
                    "input": input.map(base64::encode),
                }
            );
        }
    }
}
