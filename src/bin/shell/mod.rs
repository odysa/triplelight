//! Stateful local SPARQL shell. Storage, parsing and execution use the embedded engine.
mod output;

use std::error::Error;
use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Instant;
use triplelight::embedded::Database;
use triplelight::sparql::{parse_query, parse_update, statement_end};

use output::Mode;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str =
    "usage: triplelight shell [STORE|:memory:] [-c SPARQL] [--mode box|tsv|csv|json]";
const HELP: &str = "\
.help             Show shell commands
.open [STORE]     Open or create a store, or start a fresh in-memory session
.import FILE      Load .nt, .ttl, .nq or .trig into the current store
.read FILE        Run a script with SPARQL and shell commands
.mode MODE        Use box, tsv, csv or json for SELECT results
.timer on|off     Show execution time on stderr
.quit             Exit the shell
End a SPARQL statement with a top-level semicolon. End of input also runs any pending statement.";

pub(super) fn run(args: &[String]) {
    if let Err(error) = start(args)
        && !is_broken_pipe(error.as_ref())
    {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn start(args: &[String]) -> Result<()> {
    let mut path = None;
    let mut command = None;
    let mut mode = None;
    let mut args = args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        match arg {
            "-h" | "--help" => {
                println!("{USAGE}\n\n{HELP}");
                return Ok(());
            }
            "-c" | "--command" if command.is_none() => command = Some(args.next().ok_or(USAGE)?),
            "--mode" if mode.is_none() => mode = Some(args.next().ok_or(USAGE)?.parse()?),
            arg if !arg.starts_with('-') && path.is_none() => path = Some(arg),
            _ => return Err(USAGE.into()),
        }
    }
    let interactive = command.is_none() && io::stdin().is_terminal() && io::stdout().is_terminal();
    let mut shell = Shell {
        db: Database::in_memory(),
        directory: None,
        mode: mode.unwrap_or(if interactive { Mode::Box } else { Mode::Tsv }),
        timer: false,
    };
    if let Some(path) = path.filter(|&path| path != ":memory:") {
        shell.open(path)?;
    }
    if let Some(command) = command {
        return shell.execute(command);
    }
    if interactive {
        shell.interactive()
    } else {
        shell.script(io::stdin().lock(), 0).map(|_| ())
    }
}

struct Shell {
    db: Database,
    directory: Option<PathBuf>,
    mode: Mode,
    timer: bool,
}

impl Shell {
    fn open(&mut self, path: &str) -> Result<()> {
        if path == ":memory:" {
            self.db = Database::in_memory();
            self.directory = None;
        } else {
            let directory = PathBuf::from(path);
            let canonical = directory.canonicalize().ok();
            if canonical.is_some() && canonical == self.directory {
                return Ok(());
            }
            self.db = Database::open(&directory)?;
            self.directory = canonical.or_else(|| directory.canonicalize().ok());
        }
        Ok(())
    }

    fn execute(&mut self, request: &str) -> Result<()> {
        if !has_content(request) {
            return Ok(());
        }
        // The shell's optional final semicolon is not part of SPARQL Query syntax.
        // Feed Update the original text so -c can execute a multi-operation request atomically.
        let query = statement_end(request)
            .filter(|&end| !has_content(&request[end + 1..]))
            .map_or(request, |end| &request[..end]);
        let started = Instant::now();
        let query_error = match parse_query(query, None) {
            Ok(_) => {
                let result = self.db.query(query)?;
                self.print_time(started);
                return self
                    .mode
                    .write(&result, &mut io::stdout().lock())
                    .map_err(Into::into);
            }
            Err(error) => error,
        };
        match parse_update(request, None) {
            Ok(update) if !update.operations.is_empty() => {
                let changed = self.db.update(request)?;
                self.print_time(started);
                eprintln!("{changed} triples changed");
                Ok(())
            }
            Err(error) if (error.line, error.column) > (query_error.line, query_error.column) => {
                Err(error.into())
            }
            _ => Err(query_error.into()),
        }
    }

    fn print_time(&self, started: Instant) {
        if self.timer {
            eprintln!("Time: {:.3} ms", started.elapsed().as_secs_f64() * 1000.0);
        }
    }

    fn dot(&mut self, text: &str, depth: usize) -> Result<bool> {
        let (command, arg) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        let arg = arg.trim();
        match (command, arg) {
            (".help", "") => writeln!(io::stdout().lock(), "{HELP}")?,
            (".quit" | ".exit", "") => return Ok(true),
            (".open", "") => self.open(":memory:")?,
            (".open", path) => self.open(path_arg(path)?)?,
            (".import", "") => return Err("usage: .import FILE".into()),
            (".import", path) => {
                let count = self.db.load(path_arg(path)?)?;
                eprintln!("{count} triples loaded");
            }
            (".mode", value) => self.mode = value.parse()?,
            (".timer", "on") => self.timer = true,
            (".timer", "off") => self.timer = false,
            (".timer", _) => return Err("usage: .timer on|off".into()),
            (".read", "") => return Err("usage: .read FILE".into()),
            (".read", _) if depth >= 8 => return Err(".read nesting limit exceeded".into()),
            (".read", path) => {
                let path = path_arg(path)?;
                let file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
                return self
                    .script(BufReader::new(file), depth + 1)
                    .map_err(|error| with_context(error, path));
            }
            _ => {
                return Err(format!(
                    "unknown command or unexpected arguments: {command}; use .help"
                )
                .into());
            }
        }
        Ok(false)
    }

    fn line(&mut self, pending: &mut String, line: &str, depth: usize) -> Result<bool> {
        if pending.is_empty() && line.trim_start().starts_with('.') {
            return self.dot(line.trim(), depth);
        }
        pending.push_str(line.trim_end_matches(['\r', '\n']));
        pending.push('\n');

        if line.contains(';') {
            let mut rest = pending.as_str();
            while let Some(end) = statement_end(rest) {
                if let Err(error) = self.execute(&rest[..end]) {
                    pending.clear();
                    return Err(error);
                }
                rest = &rest[end + 1..];
            }
            pending.drain(..pending.len() - rest.len());
        }
        if !has_content(pending) {
            pending.clear();
        }
        Ok(false)
    }

    fn script(&mut self, input: impl BufRead, depth: usize) -> Result<bool> {
        let mut pending = String::new();
        for (line_number, line) in input.lines().enumerate() {
            if self
                .line(&mut pending, &line?, depth)
                .map_err(|error| with_context(error, &format!("line {}", line_number + 1)))?
            {
                return Ok(true);
            }
        }
        self.execute(&pending)
            .map_err(|error| with_context(error, "end of input"))?;
        Ok(false)
    }

    fn interactive(&mut self) -> Result<()> {
        eprintln!("TripleLight SPARQL shell. Enter .help for commands.");
        #[cfg(feature = "shell-editing")]
        let mut editor = rustyline::DefaultEditor::new()?;
        #[cfg(not(feature = "shell-editing"))]
        let mut input = io::stdin().lock();
        let mut pending = String::new();
        loop {
            let prompt = if pending.is_empty() {
                "sparql> "
            } else {
                "   ...> "
            };
            #[cfg(feature = "shell-editing")]
            let line = match editor.readline(prompt) {
                Ok(line) => {
                    editor.add_history_entry(line.as_str())?;
                    line
                }
                Err(rustyline::error::ReadlineError::Interrupted) => {
                    pending.clear();
                    continue;
                }
                Err(rustyline::error::ReadlineError::Eof) => break,
                Err(e) => return Err(e.into()),
            };
            #[cfg(not(feature = "shell-editing"))]
            let line = {
                print!("{prompt}");
                io::stdout().flush()?;
                let mut line = String::new();
                if input.read_line(&mut line)? == 0 {
                    break;
                }
                line
            };
            match self.line(&mut pending, &line, 0) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) if is_broken_pipe(error.as_ref()) => return Err(error),
                Err(e) => eprintln!("error: {e}"),
            }
        }
        self.execute(&pending)
    }
}

fn is_broken_pipe(error: &(dyn Error + 'static)) -> bool {
    error
        .downcast_ref::<io::Error>()
        .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe)
}

fn with_context(error: Box<dyn Error>, context: &str) -> Box<dyn Error> {
    if is_broken_pipe(error.as_ref()) {
        error
    } else {
        format!("{context}: {error}").into()
    }
}

fn has_content(text: &str) -> bool {
    text.lines()
        .map(str::trim_start)
        .any(|line| !line.is_empty() && !line.starts_with('#'))
}

fn path_arg(arg: &str) -> Result<&str> {
    if let Some(quote @ ('\'' | '"')) = arg.chars().next() {
        return arg[1..]
            .strip_suffix(quote)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| "expected a quoted path".into());
    }
    Ok(arg)
}
