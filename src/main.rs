use clap::{ArgGroup, Parser, Subcommand};
use mmagolf::Command;
use std::{
    fs::read_to_string,
    io::{Read, Write},
    process::{self, exit, Stdio},
};

#[derive(Debug, Parser)]
#[clap(version, about, long_about = None)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// submit code
    #[clap(
        arg_required_else_help = true,
        group(
            ArgGroup::new("source")
                .required(true)
                .args(&["file", "code"]),
        ),
    )]
    Submit {
        /// source file
        #[clap(short, long)]
        file: Option<String>,
        /// source code
        #[clap(short, long)]
        code: Option<String>,
        /// language
        #[clap(short, long)]
        lang: String,
        #[clap(short, long)]
        problem_name: String,
        #[clap(short, long)]
        dry_run: bool,
        #[clap(short, long)]
        retain_eof_newline: bool,
    },
    /// run the code in the judge surver to see if the code works
    #[clap(
        arg_required_else_help = true,
        override_usage = "echo <INPUT> | mmagolf codetest --lang <LANG> <--file <FILE>|--code <CODE>>",
        group(
            ArgGroup::new("source")
                .required(true)
                .args(&["file", "code"]),
        ),
    )]
    Codetest {
        /// source file
        #[clap(short, long)]
        file: Option<String>,
        /// source code
        #[clap(short, long)]
        code: Option<String>,
        /// language
        #[clap(short, long)]
        lang: String,
    },
}

#[cfg(debug_assertions)]
const MMAGOLF_BACK: &str = "target/debug/mmagolf-back";
#[cfg(not(debug_assertions))]
const MMAGOLF_BACK: &str = "/home/mado/.cargo/bin/mmagolf-back";

fn main() {
    let args = Cli::parse();
    let command: Command = args.into();
    let command = serde_json::to_string(&command).unwrap();
    let mut back = process::Command::new(MMAGOLF_BACK)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .spawn()
        .unwrap();
    back.stdin
        .as_mut()
        .unwrap()
        .write_all(command.as_bytes())
        .unwrap();
    back.wait_with_output().unwrap();
}

impl From<Cli> for Command {
    fn from(cli: Cli) -> Self {
        match cli.command {
            Commands::Submit {
                file,
                code,
                lang,
                problem_name,
                dry_run,
                retain_eof_newline,
            } => Command::Submit {
                code: code_or_file(code, file, retain_eof_newline),
                lang,
                problem_name,
                dry_run,
            },
            Commands::Codetest { file, code, lang } => Command::Codetest {
                code: code_or_file(code, file, true),
                lang,
                input: if atty::is(atty::Stream::Stdin) {
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
                        base64::encode(input)
                    })
                },
            },
        }
    }
}

fn code_or_file(code: Option<String>, file: Option<String>, retain_eof_newline: bool) -> String {
    let mut s = match (file, code) {
        (None, Some(code)) => code,
        (Some(file), None) => read_to_string(&file).unwrap_or_else(|e| {
            eprintln!("{}: {}", file, e);
            exit(1)
        }),
        _ => panic!(),
    };
    if !retain_eof_newline && s.ends_with('\n') {
        s.pop();
        s
    } else {
        s
    }
}
