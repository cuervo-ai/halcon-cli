# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-02-14

### Added

#### Core Features
- Initial release of Cuervo CLI - AI-powered terminal assistant
- Multi-provider support (Anthropic Claude, OpenAI, DeepSeek, Ollama)
- Interactive REPL with rich terminal UI
- Full-featured TUI mode with multi-panel interface
- Model Context Protocol (MCP) integration
- Comprehensive tool system (file operations, git, directory tree, etc.)

#### Architecture
- Modular workspace architecture with 14 crates
- Async-first design with Tokio runtime
- Event-driven orchestration system
- Context management with automatic summarization
- Semantic memory with vector storage
- Audit logging and provenance tracking

#### TUI/UX
- Three-zone layout (Prompt, Activity, Status)
- Syntax highlighting for code blocks
- Real-time token usage and cost tracking
- Overlay system (Command Palette, Search, Help)
- Adaptive theming with color science (Momoto integration)
- Keyboard shortcuts and vim-style navigation
- Circuit breaker for API rate limiting
- Graceful degradation and error recovery

#### Security
- PII detection and redaction
- Sandbox mode for tool execution
- Dry-run mode for testing
- Keyring integration for secure credential storage
- Audit trail for all AI interactions
- Configurable safety guardrails

#### Distribution System
- One-line installation for Linux/macOS/Windows
- Automated cross-platform binary releases (6 targets)
- SHA256 checksum verification
- Automatic PATH configuration
- Fallback installation methods (cargo-binstall, cargo install)
- GitHub Actions CI/CD pipeline
- Comprehensive installation documentation

#### Documentation
- Quick start guide (5-minute setup)
- Complete installation guide with troubleshooting
- Visual installation examples
- Release process documentation
- Testing and validation guides
- API documentation and examples

#### Testing
- 1486+ passing tests across workspace
- Integration tests for core functionality
- TUI component tests
- Tool audit tests
- Installation script validation

### Technical Details

**Supported Platforms:**
- Linux x86_64 (glibc)
- Linux x86_64 (musl/Alpine)
- Linux ARM64
- macOS Intel (x86_64)
- macOS Apple Silicon (M1/M2/M3/M4)
- Windows x64

**Performance:**
- Optimized release builds (LTO, strip, size optimization)
- Lazy loading of heavy dependencies
- Streaming responses for real-time output
- Efficient context window management

**Developer Experience:**
- Hot-reloadable configuration
- Extensive logging with tracing
- Developer tools (stress tests, replay runner)
- Modular architecture for easy extension

---

[0.1.0]: https://github.com/cuervo-ai/cuervo-cli/releases/tag/v0.1.0
