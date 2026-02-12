/// Result of handling a REPL slash-command.
#[derive(Debug)]
pub enum CommandResult {
    /// Command handled, continue the REPL loop.
    Continue,
    /// User requested exit.
    Exit,
    /// Unrecognized command.
    Unknown(String),
    /// List recent sessions.
    SessionList,
    /// Show current session info.
    SessionShow,
    /// Run a diagnostic test.
    TestRun(TestKind),
    /// Orchestrate a multi-agent task.
    Orchestrate(String),
    /// Run a prompt in dry-run mode (no destructive tools executed).
    DryRun(String),
    /// Show trace context info for the current session.
    TraceInfo,
    /// Show agent state machine info.
    StateInfo,

    // --- Phase 19: Agent Operating Console ---

    /// Spawn parallel research agents for a query.
    Research(String),
    /// Inspect a subsystem.
    Inspect(InspectTarget),
    /// Generate an execution plan from a goal description.
    Plan(String),
    /// Execute a saved plan by plan_id.
    RunPlan(String),
    /// Resume a session from its latest checkpoint.
    Resume(String),
    /// Cancel an agent task.
    Cancel(String),
    /// Show live system status (session, providers, health).
    LiveStatus,
    /// Show logs/trace timeline, optionally filtered by task_id.
    Logs(Option<String>),
    /// Show aggregated metrics (tokens, cost, cache).
    Metrics,
    /// Browse trace steps for a session.
    TraceBrowse(Option<String>),
    /// Deterministic replay of a recorded session.
    Replay(String),
    /// Step through trace (forward/back).
    Step(StepDirection),
    /// Create a checkpoint of the current session.
    Snapshot,
    /// Compare two sessions side-by-side.
    Diff(String, String),
    /// Run a performance benchmark.
    Benchmark(String),
    /// Analyze metrics and identify bottlenecks.
    Optimize,
    /// Deep analysis of model/tool statistics.
    Analyze,
}

/// Subsystem targets for /inspect.
#[derive(Debug, Clone, PartialEq)]
pub enum InspectTarget {
    Runtime,
    Memory,
    Db,
    Traces,
}

/// Direction for /step command.
#[derive(Debug, Clone, PartialEq)]
pub enum StepDirection {
    Forward,
    Back,
}

/// Kinds of self-diagnostic tests available via /test.
#[derive(Debug, Clone, PartialEq)]
pub enum TestKind {
    /// Run provider connectivity check.
    Provider(String),
    /// Show system status diagnostics.
    Status,
}

/// Dispatch a slash-command (input without the leading `/`).
pub fn handle(input: &str, provider: &str, model: &str) -> CommandResult {
    let (cmd, args) = input.split_once(' ').unwrap_or((input, ""));

    match cmd {
        "quit" | "exit" | "q" => {
            println!("Goodbye!");
            CommandResult::Exit
        }
        "help" | "h" | "?" => {
            print_help();
            CommandResult::Continue
        }
        "clear" => {
            print!("\x1B[2J\x1B[1;1H");
            CommandResult::Continue
        }
        "model" => {
            println!("Current: {provider}/{model}");
            CommandResult::Continue
        }
        "session" => handle_session(args),
        "test" => handle_test(args, provider),
        "orchestrate" | "orch" => handle_orchestrate(args),
        "dry-run" | "dryrun" => handle_dry_run(args),
        "trace" => handle_trace(args),
        "state" => {
            CommandResult::StateInfo
        }

        // --- Phase 19: Agent Operating Console ---
        "research" | "res" => handle_research(args),
        "inspect" => handle_inspect(args),
        "plan" => handle_plan(args),
        "run" => handle_run(args),
        "resume" => handle_resume(args),
        "cancel" => handle_cancel(args),
        "status" => CommandResult::LiveStatus,
        "logs" => handle_logs(args),
        "metrics" => CommandResult::Metrics,
        "replay" => handle_replay(args),
        "step" => handle_step(args),
        "snapshot" => CommandResult::Snapshot,
        "diff" => handle_diff(args),
        "benchmark" | "bench" => handle_benchmark(args),
        "optimize" | "opt" => CommandResult::Optimize,
        "analyze" => CommandResult::Analyze,

        _ => CommandResult::Unknown(cmd.to_string()),
    }
}

fn handle_session(args: &str) -> CommandResult {
    let sub = args.trim();
    match sub {
        "list" | "ls" => CommandResult::SessionList,
        "" | "show" | "info" => CommandResult::SessionShow,
        _ => {
            println!("Usage: /session [list|show]");
            CommandResult::Continue
        }
    }
}

fn handle_test(args: &str, current_provider: &str) -> CommandResult {
    let sub = args.trim();
    match sub {
        "" | "status" => CommandResult::TestRun(TestKind::Status),
        "provider" => CommandResult::TestRun(TestKind::Provider(current_provider.to_string())),
        _ if sub.starts_with("provider ") => {
            let name = sub.strip_prefix("provider ").unwrap().trim();
            CommandResult::TestRun(TestKind::Provider(name.to_string()))
        }
        _ => {
            println!(
                "\
Usage:
  /test               Run all diagnostics
  /test status        System status check
  /test provider      Test current provider
  /test provider <n>  Test a specific provider (echo, anthropic)"
            );
            CommandResult::Continue
        }
    }
}

fn handle_orchestrate(args: &str) -> CommandResult {
    let instruction = args.trim();
    if instruction.is_empty() {
        println!("Usage: /orchestrate <instruction>");
        println!("  Decomposes the instruction into sub-agent tasks and runs them concurrently.");
        return CommandResult::Continue;
    }
    CommandResult::Orchestrate(instruction.to_string())
}

fn handle_dry_run(args: &str) -> CommandResult {
    let prompt = args.trim();
    if prompt.is_empty() {
        println!("Usage: /dry-run <prompt>");
        println!("  Runs the prompt without executing destructive tools.");
        return CommandResult::Continue;
    }
    CommandResult::DryRun(prompt.to_string())
}

fn handle_trace(args: &str) -> CommandResult {
    let sub = args.trim();
    if sub.is_empty() {
        return CommandResult::TraceInfo;
    }
    CommandResult::TraceBrowse(Some(sub.to_string()))
}

fn handle_research(args: &str) -> CommandResult {
    let query = args.trim();
    if query.is_empty() {
        println!("Usage: /research <query>");
        println!("  Spawns parallel agents to research a topic.");
        return CommandResult::Continue;
    }
    CommandResult::Research(query.to_string())
}

fn handle_inspect(args: &str) -> CommandResult {
    match args.trim() {
        "runtime" | "rt" => CommandResult::Inspect(InspectTarget::Runtime),
        "memory" | "mem" => CommandResult::Inspect(InspectTarget::Memory),
        "db" | "database" => CommandResult::Inspect(InspectTarget::Db),
        "traces" | "trace" => CommandResult::Inspect(InspectTarget::Traces),
        "" => {
            println!("Usage: /inspect <target>");
            println!("  Targets: runtime, memory, db, traces");
            CommandResult::Continue
        }
        other => {
            println!("Unknown inspect target '{other}'");
            println!("  Targets: runtime, memory, db, traces");
            CommandResult::Continue
        }
    }
}

fn handle_plan(args: &str) -> CommandResult {
    let goal = args.trim();
    if goal.is_empty() {
        println!("Usage: /plan <goal>");
        println!("  Generates an execution plan via LLM.");
        return CommandResult::Continue;
    }
    CommandResult::Plan(goal.to_string())
}

fn handle_run(args: &str) -> CommandResult {
    let plan_id = args.trim();
    if plan_id.is_empty() {
        println!("Usage: /run <plan_id>");
        println!("  Executes a saved plan via the orchestrator.");
        return CommandResult::Continue;
    }
    CommandResult::RunPlan(plan_id.to_string())
}

fn handle_resume(args: &str) -> CommandResult {
    let session_id = args.trim();
    if session_id.is_empty() {
        println!("Usage: /resume <session_id>");
        println!("  Resumes from the latest checkpoint of a session.");
        return CommandResult::Continue;
    }
    CommandResult::Resume(session_id.to_string())
}

fn handle_cancel(args: &str) -> CommandResult {
    let task_id = args.trim();
    if task_id.is_empty() {
        println!("Usage: /cancel <task_id>");
        println!("  Cancels an agent task.");
        return CommandResult::Continue;
    }
    CommandResult::Cancel(task_id.to_string())
}

fn handle_logs(args: &str) -> CommandResult {
    let filter = args.trim();
    if filter.is_empty() {
        CommandResult::Logs(None)
    } else {
        CommandResult::Logs(Some(filter.to_string()))
    }
}

fn handle_replay(args: &str) -> CommandResult {
    let session_id = args.trim();
    if session_id.is_empty() {
        println!("Usage: /replay <session_id>");
        println!("  Deterministic replay of a recorded session.");
        return CommandResult::Continue;
    }
    CommandResult::Replay(session_id.to_string())
}

fn handle_step(args: &str) -> CommandResult {
    match args.trim() {
        "" | "forward" | "fwd" | "next" | "n" => CommandResult::Step(StepDirection::Forward),
        "back" | "prev" | "b" | "p" => CommandResult::Step(StepDirection::Back),
        _ => {
            println!("Usage: /step [forward|back]");
            CommandResult::Continue
        }
    }
}

fn handle_diff(args: &str) -> CommandResult {
    let parts: Vec<&str> = args.split_whitespace().collect();
    if parts.len() != 2 {
        println!("Usage: /diff <session_a> <session_b>");
        println!("  Compares two sessions side-by-side.");
        return CommandResult::Continue;
    }
    CommandResult::Diff(parts[0].to_string(), parts[1].to_string())
}

fn handle_benchmark(args: &str) -> CommandResult {
    let workload = args.trim();
    if workload.is_empty() {
        println!("Usage: /benchmark <workload>");
        println!("  Workloads: inference, tools, cache, full");
        return CommandResult::Continue;
    }
    CommandResult::Benchmark(workload.to_string())
}

fn print_help() {
    println!(
        "\
Research:
  /research <query>   Spawn parallel agents to research a topic
  /inspect <target>   Inspect subsystem (runtime, memory, db, traces)

Plan & Execute:
  /plan <goal>        Generate an execution plan via LLM
  /run <plan_id>      Execute a saved plan via orchestrator
  /orchestrate <msg>  Run multi-agent orchestration
  /dry-run <prompt>   Run without destructive tool execution
  /resume <id>        Resume from latest checkpoint
  /cancel <task_id>   Cancel an agent task

Observe:
  /status             Live system status (session, providers, health)
  /metrics            Aggregated metrics (tokens, cost, cache)
  /logs [task_id]     Trace timeline / task logs
  /trace [id]         Trace context info / browse session traces

Debug:
  /replay <id>        Deterministic replay of a recorded session
  /step [fwd|back]    Step through trace steps
  /snapshot           Create checkpoint of current session
  /diff <id_a> <id_b> Compare two sessions side-by-side

Self-Improve:
  /benchmark <type>   Run performance benchmark (inference, tools, cache, full)
  /optimize           Analyze metrics and suggest optimizations
  /analyze            Deep analysis of model/tool statistics

Session:
  /model              Show current provider/model
  /session list       List recent sessions
  /session show       Show current session info
  /state              Show agent state machine info
  /test [provider]    Run self-diagnostics

Navigation:
  /help, /?           Show this help
  /clear              Clear the screen
  /quit, /q           Exit cuervo

Keyboard:
  Enter               Send message
  Alt+Enter           Insert newline (multi-line input)
  Ctrl+C              Cancel current input
  Ctrl+D              Exit
  Ctrl+R              Search history
  Up/Down             Navigate history

CLI Commands (run from shell):
  cuervo chat \"msg\"   Single-shot mode
  cuervo chat -r ID   Resume a session
  cuervo doctor       Runtime health diagnostics
  cuervo auth login   Configure API keys"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_returns_exit() {
        assert!(matches!(handle("quit", "p", "m"), CommandResult::Exit));
        assert!(matches!(handle("exit", "p", "m"), CommandResult::Exit));
        assert!(matches!(handle("q", "p", "m"), CommandResult::Exit));
    }

    #[test]
    fn help_returns_continue() {
        assert!(matches!(handle("help", "p", "m"), CommandResult::Continue));
        assert!(matches!(handle("h", "p", "m"), CommandResult::Continue));
        assert!(matches!(handle("?", "p", "m"), CommandResult::Continue));
    }

    #[test]
    fn clear_returns_continue() {
        assert!(matches!(handle("clear", "p", "m"), CommandResult::Continue));
    }

    #[test]
    fn model_returns_continue() {
        assert!(matches!(handle("model", "p", "m"), CommandResult::Continue));
    }

    #[test]
    fn unknown_returns_unknown() {
        match handle("foobar", "p", "m") {
            CommandResult::Unknown(cmd) => assert_eq!(cmd, "foobar"),
            _ => panic!("Expected Unknown"),
        }
    }

    #[test]
    fn command_with_args_parses_correctly() {
        assert!(matches!(
            handle("help extra args", "p", "m"),
            CommandResult::Continue
        ));
    }

    #[test]
    fn session_list_command() {
        assert!(matches!(
            handle("session list", "p", "m"),
            CommandResult::SessionList
        ));
        assert!(matches!(
            handle("session ls", "p", "m"),
            CommandResult::SessionList
        ));
    }

    #[test]
    fn session_show_command() {
        assert!(matches!(
            handle("session show", "p", "m"),
            CommandResult::SessionShow
        ));
        assert!(matches!(
            handle("session", "p", "m"),
            CommandResult::SessionShow
        ));
    }

    #[test]
    fn session_unknown_sub_continues() {
        assert!(matches!(
            handle("session foo", "p", "m"),
            CommandResult::Continue
        ));
    }

    #[test]
    fn test_status_command() {
        match handle("test", "echo", "m") {
            CommandResult::TestRun(TestKind::Status) => {}
            other => panic!("Expected TestRun(Status), got {other:?}"),
        }
        match handle("test status", "echo", "m") {
            CommandResult::TestRun(TestKind::Status) => {}
            other => panic!("Expected TestRun(Status), got {other:?}"),
        }
    }

    #[test]
    fn test_provider_command() {
        match handle("test provider", "echo", "m") {
            CommandResult::TestRun(TestKind::Provider(p)) => assert_eq!(p, "echo"),
            other => panic!("Expected TestRun(Provider), got {other:?}"),
        }
    }

    #[test]
    fn test_provider_named() {
        match handle("test provider anthropic", "echo", "m") {
            CommandResult::TestRun(TestKind::Provider(p)) => assert_eq!(p, "anthropic"),
            other => panic!("Expected TestRun(Provider(anthropic)), got {other:?}"),
        }
    }

    #[test]
    fn test_unknown_sub_prints_usage() {
        assert!(matches!(
            handle("test unknown", "p", "m"),
            CommandResult::Continue
        ));
    }

    #[test]
    fn orchestrate_command_with_instruction() {
        match handle("orchestrate do something cool", "p", "m") {
            CommandResult::Orchestrate(instruction) => {
                assert_eq!(instruction, "do something cool");
            }
            other => panic!("Expected Orchestrate, got {other:?}"),
        }
    }

    #[test]
    fn orchestrate_shorthand() {
        match handle("orch run tests", "p", "m") {
            CommandResult::Orchestrate(instruction) => {
                assert_eq!(instruction, "run tests");
            }
            other => panic!("Expected Orchestrate, got {other:?}"),
        }
    }

    #[test]
    fn orchestrate_no_args_shows_usage() {
        assert!(matches!(
            handle("orchestrate", "p", "m"),
            CommandResult::Continue
        ));
        assert!(matches!(
            handle("orchestrate  ", "p", "m"),
            CommandResult::Continue
        ));
    }

    #[test]
    fn command_dry_run_parsing() {
        match handle("dry-run list all files", "p", "m") {
            CommandResult::DryRun(prompt) => assert_eq!(prompt, "list all files"),
            other => panic!("Expected DryRun, got {other:?}"),
        }
        // Shorthand
        match handle("dryrun test something", "p", "m") {
            CommandResult::DryRun(prompt) => assert_eq!(prompt, "test something"),
            other => panic!("Expected DryRun, got {other:?}"),
        }
    }

    #[test]
    fn command_dry_run_empty_shows_usage() {
        assert!(matches!(
            handle("dry-run", "p", "m"),
            CommandResult::Continue
        ));
        assert!(matches!(
            handle("dry-run  ", "p", "m"),
            CommandResult::Continue
        ));
    }

    #[test]
    fn command_trace_parsing() {
        assert!(matches!(
            handle("trace", "p", "m"),
            CommandResult::TraceInfo
        ));
    }

    #[test]
    fn command_state_parsing() {
        assert!(matches!(
            handle("state", "p", "m"),
            CommandResult::StateInfo
        ));
    }

    // --- Phase 19: Console command parsing tests ---

    #[test]
    fn research_command() {
        match handle("research how does auth work", "p", "m") {
            CommandResult::Research(q) => assert_eq!(q, "how does auth work"),
            other => panic!("Expected Research, got {other:?}"),
        }
    }

    #[test]
    fn research_alias() {
        match handle("res find patterns", "p", "m") {
            CommandResult::Research(q) => assert_eq!(q, "find patterns"),
            other => panic!("Expected Research, got {other:?}"),
        }
    }

    #[test]
    fn research_empty_shows_usage() {
        assert!(matches!(
            handle("research", "p", "m"),
            CommandResult::Continue
        ));
    }

    #[test]
    fn inspect_targets() {
        assert!(matches!(
            handle("inspect runtime", "p", "m"),
            CommandResult::Inspect(InspectTarget::Runtime)
        ));
        assert!(matches!(
            handle("inspect memory", "p", "m"),
            CommandResult::Inspect(InspectTarget::Memory)
        ));
        assert!(matches!(
            handle("inspect db", "p", "m"),
            CommandResult::Inspect(InspectTarget::Db)
        ));
        assert!(matches!(
            handle("inspect traces", "p", "m"),
            CommandResult::Inspect(InspectTarget::Traces)
        ));
    }

    #[test]
    fn inspect_aliases() {
        assert!(matches!(
            handle("inspect rt", "p", "m"),
            CommandResult::Inspect(InspectTarget::Runtime)
        ));
        assert!(matches!(
            handle("inspect mem", "p", "m"),
            CommandResult::Inspect(InspectTarget::Memory)
        ));
        assert!(matches!(
            handle("inspect database", "p", "m"),
            CommandResult::Inspect(InspectTarget::Db)
        ));
        assert!(matches!(
            handle("inspect trace", "p", "m"),
            CommandResult::Inspect(InspectTarget::Traces)
        ));
    }

    #[test]
    fn inspect_empty_shows_usage() {
        assert!(matches!(
            handle("inspect", "p", "m"),
            CommandResult::Continue
        ));
    }

    #[test]
    fn plan_command() {
        match handle("plan fix the authentication bug", "p", "m") {
            CommandResult::Plan(g) => assert_eq!(g, "fix the authentication bug"),
            other => panic!("Expected Plan, got {other:?}"),
        }
    }

    #[test]
    fn plan_empty_shows_usage() {
        assert!(matches!(handle("plan", "p", "m"), CommandResult::Continue));
    }

    #[test]
    fn run_command() {
        match handle("run abc123", "p", "m") {
            CommandResult::RunPlan(id) => assert_eq!(id, "abc123"),
            other => panic!("Expected RunPlan, got {other:?}"),
        }
    }

    #[test]
    fn resume_command() {
        match handle("resume abc123", "p", "m") {
            CommandResult::Resume(id) => assert_eq!(id, "abc123"),
            other => panic!("Expected Resume, got {other:?}"),
        }
    }

    #[test]
    fn cancel_command() {
        match handle("cancel task-id", "p", "m") {
            CommandResult::Cancel(id) => assert_eq!(id, "task-id"),
            other => panic!("Expected Cancel, got {other:?}"),
        }
    }

    #[test]
    fn status_command() {
        assert!(matches!(
            handle("status", "p", "m"),
            CommandResult::LiveStatus
        ));
    }

    #[test]
    fn logs_command() {
        assert!(matches!(handle("logs", "p", "m"), CommandResult::Logs(None)));
        match handle("logs task-123", "p", "m") {
            CommandResult::Logs(Some(f)) => assert_eq!(f, "task-123"),
            other => panic!("Expected Logs(Some), got {other:?}"),
        }
    }

    #[test]
    fn metrics_command() {
        assert!(matches!(handle("metrics", "p", "m"), CommandResult::Metrics));
    }

    #[test]
    fn trace_browse_command() {
        // No args => TraceInfo (backward compat)
        assert!(matches!(handle("trace", "p", "m"), CommandResult::TraceInfo));
        // With session id => TraceBrowse
        match handle("trace abc123", "p", "m") {
            CommandResult::TraceBrowse(Some(id)) => assert_eq!(id, "abc123"),
            other => panic!("Expected TraceBrowse, got {other:?}"),
        }
    }

    #[test]
    fn replay_command() {
        match handle("replay abc123", "p", "m") {
            CommandResult::Replay(id) => assert_eq!(id, "abc123"),
            other => panic!("Expected Replay, got {other:?}"),
        }
    }

    #[test]
    fn replay_empty_shows_usage() {
        assert!(matches!(
            handle("replay", "p", "m"),
            CommandResult::Continue
        ));
    }

    #[test]
    fn step_command() {
        assert!(matches!(
            handle("step", "p", "m"),
            CommandResult::Step(StepDirection::Forward)
        ));
        assert!(matches!(
            handle("step forward", "p", "m"),
            CommandResult::Step(StepDirection::Forward)
        ));
        assert!(matches!(
            handle("step fwd", "p", "m"),
            CommandResult::Step(StepDirection::Forward)
        ));
        assert!(matches!(
            handle("step back", "p", "m"),
            CommandResult::Step(StepDirection::Back)
        ));
        assert!(matches!(
            handle("step prev", "p", "m"),
            CommandResult::Step(StepDirection::Back)
        ));
    }

    #[test]
    fn snapshot_command() {
        assert!(matches!(
            handle("snapshot", "p", "m"),
            CommandResult::Snapshot
        ));
    }

    #[test]
    fn diff_command() {
        match handle("diff aaa bbb", "p", "m") {
            CommandResult::Diff(a, b) => {
                assert_eq!(a, "aaa");
                assert_eq!(b, "bbb");
            }
            other => panic!("Expected Diff, got {other:?}"),
        }
    }

    #[test]
    fn diff_wrong_args_shows_usage() {
        assert!(matches!(handle("diff", "p", "m"), CommandResult::Continue));
        assert!(matches!(
            handle("diff only_one", "p", "m"),
            CommandResult::Continue
        ));
    }

    #[test]
    fn benchmark_command() {
        match handle("benchmark inference", "p", "m") {
            CommandResult::Benchmark(w) => assert_eq!(w, "inference"),
            other => panic!("Expected Benchmark, got {other:?}"),
        }
    }

    #[test]
    fn benchmark_alias() {
        match handle("bench cache", "p", "m") {
            CommandResult::Benchmark(w) => assert_eq!(w, "cache"),
            other => panic!("Expected Benchmark, got {other:?}"),
        }
    }

    #[test]
    fn benchmark_empty_shows_usage() {
        assert!(matches!(
            handle("benchmark", "p", "m"),
            CommandResult::Continue
        ));
    }

    #[test]
    fn optimize_command() {
        assert!(matches!(
            handle("optimize", "p", "m"),
            CommandResult::Optimize
        ));
        assert!(matches!(handle("opt", "p", "m"), CommandResult::Optimize));
    }

    #[test]
    fn analyze_command() {
        assert!(matches!(
            handle("analyze", "p", "m"),
            CommandResult::Analyze
        ));
    }
}
