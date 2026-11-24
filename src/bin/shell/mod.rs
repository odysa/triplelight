//! Stateful local SPARQL shell. Storage, parsing and execution use the embedded engine.
mod output;

use std::error::Error;
use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;
use triplelight::embedded::Database;
use triplelight::exec::QueryResult;
use triplelight::sparql::{parse_query, parse_update, statement_end};

use output::Mode;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "\
usage: triplelight [STORE|:memory:] [OPTIONS]
       triplelight shell [STORE|:memory:] [OPTIONS]

-c COMMAND        Execute a SPARQL request or dot command, then exit
-cmd COMMAND      Execute a command before reading stdin
-f FILE           Run a script, then exit
-init FILE        Run a startup script before other commands
-batch            Read input without prompts
-box -tsv -csv -json
                  Change output mode for following commands
-help -version    Show help or version

Other commands: query, explain, validate, build-disk, verify, text-index, serve, serve-store.";
const HELP: &str = "\
.help             Show shell commands
.open [STORE]     Open or create a store, or start a fresh in-memory session
.import FILE      Load .nt, .ttl, .nq or .trig into the current store
.read FILE        Run a script with SPARQL and shell commands
.mode MODE        Use box, tsv, csv or json for SELECT results
.output [FILE]    Redirect results to a file, or restore stdout
.once FILE        Redirect the next statement's results
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
    let mut init = None;
    let mut actions = Vec::new();
    let mut batch = false;
    let mut exit = false;
    let mut args = args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        match arg {
            "-h" | "-help" | "--help" => {
                println!("{USAGE}\n\n{HELP}");
                return Ok(());
            }
            "-version" | "--version" => {
                println!("TripleLight {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-c" | "-s" | "--command" | "-cmd" => {
                actions.push(Action::Command(args.next().ok_or("expected a command")?));
                exit |= arg != "-cmd";
            }
            "-f" => {
                actions.push(Action::Script(args.next().ok_or("-f requires a file")?));
                exit = true;
            }
            "-init" if init.is_none() => init = Some(args.next().ok_or("-init requires a file")?),
            "-batch" => batch = true,
            "-box" | "-table" | "-tsv" | "-csv" | "-json" => {
                actions.push(Action::Mode(arg[1..].parse()?))
            }
            "--mode" => actions.push(Action::Mode(
                args.next().ok_or("--mode requires a format")?.parse()?,
            )),
            "--" if path.is_none() => {
                path = Some(args.next().ok_or("expected a store path after --")?);
                if args.next().is_some() {
                    return Err(USAGE.into());
                }
                break;
            }
            arg if !arg.starts_with('-') && path.is_none() => path = Some(arg),
            _ => return Err(USAGE.into()),
        }
    }
    let interactive = !exit && !batch && io::stdin().is_terminal() && io::stdout().is_terminal();
    let mut shell = Shell {
        db: Database::in_memory(),
        directory: None,
        mode: if interactive { Mode::Box } else { Mode::Tsv },
        timer: false,
        output: None,
        next_output: None,
    };
    if let Some(path) = path.filter(|&path| path != ":memory:") {
        shell.open(path)?;
    }
    if let Some(file) = init
        && shell.read(file, 0)?
    {
        return Ok(());
    }
    for action in actions {
        let quit = match action {
            Action::Mode(mode) => {
                shell.mode = mode;
                false
            }
            Action::Command(command) if command.trim_start().starts_with('.') => {
                shell.dot(command.trim(), 0)?
            }
            Action::Command(command) => {
                shell.execute(command)?;
                false
            }
            Action::Script(file) => shell.read(file, 0)?,
        };
        if quit {
            return Ok(());
        }
    }
    if exit {
        return Ok(());
    }
    if interactive {
        shell.interactive()
    } else {
        shell.script(io::stdin().lock(), 0).map(|_| ())
    }
}

enum Action<'a> {
    Mode(Mode),
    Command(&'a str),
    Script(&'a str),
}

struct Shell {
    db: Database,
    directory: Option<PathBuf>,
    mode: Mode,
    timer: bool,
    output: Option<File>,
    next_output: Option<File>,
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
            check_store_directory(&directory)?;
            self.db = Database::open(&directory)?;
            self.directory = canonical.or_else(|| directory.canonicalize().ok());
        }
        Ok(())
    }

    fn execute(&mut self, request: &str) -> Result<()> {
        if !has_content(request) {
            return Ok(());
        }
        let mut output = self.next_output.take();
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
                return self.write_query(&result, output.as_mut());
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

    fn write_query(&mut self, result: &QueryResult, once: Option<&mut File>) -> Result<()> {
        match once.or(self.output.as_mut()) {
            Some(file) => self.mode.write(result, file)?,
            None => self.mode.write(result, &mut io::stdout().lock())?,
        }
        Ok(())
    }

    fn print_time(&self, started: Instant) {
        if self.timer {
            eprintln!("Time: {:.3} ms", started.elapsed().as_secs_f64() * 1000.0);
        }
    }

    fn dot(&mut self, text: &str, depth: usize) -> Result<bool> {
        if text.contains(['\r', '\n']) {
            return Err("dot commands take one line; use -f or .read for scripts".into());
        }
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
            (".output", "" | "stdout") => self.output = None,
            (".output", path) => self.output = Some(File::create(path_arg(path)?)?),
            (".once", "") => return Err("usage: .once FILE".into()),
            (".once", path) => self.next_output = Some(File::create(path_arg(path)?)?),
            (".timer", "on") => self.timer = true,
            (".timer", "off") => self.timer = false,
            (".timer", _) => return Err("usage: .timer on|off".into()),
            (".read", "") => return Err("usage: .read FILE".into()),
            (".read", path) => return self.read(path_arg(path)?, depth),
            _ => {
                return Err(format!(
                    "unknown command or unexpected arguments: {command}; use .help"
                )
                .into());
            }
        }
        Ok(false)
    }

    fn read(&mut self, path: &str, depth: usize) -> Result<bool> {
        if depth >= 8 {
            return Err(".read nesting limit exceeded".into());
        }
        let file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
        self.script(BufReader::new(file), depth + 1)
            .map_err(|error| with_context(error, path))
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

fn check_store_directory(path: &Path) -> Result<()> {
    if !path.is_dir() || path.join("CURRENT").is_file() || path.join("MANIFEST").is_file() {
        return Ok(());
    }
    // Permit the files left by an interrupted first open. Avoid initializing an unrelated
    // directory since store recovery can delete uncommitted base directories.
    let has_lock = path.join("LOCK").is_file();
    for entry in path.read_dir()? {
        let name = entry?.file_name();
        if !has_lock || !matches!(name.to_str(), Some("LOCK" | "CURRENT.tmp" | "base-0")) {
            return Err(format!(
                "{} is not an empty directory or a TripleLight store",
                path.display()
            )
            .into());
        }
    }
    Ok(())
}
