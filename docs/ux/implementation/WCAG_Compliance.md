# WCAG 2.2 Compliance Report — Cuervo CLI

**Project:** Cuervo CLI (Terminal Application)
**Date:** February 2026
**Standard:** WCAG 2.2 Level AA (with AAA aspirations)
**Author:** UX Research & Product Design Team

---

## Executive Summary

WCAG 2.2 was designed primarily for web content but its principles (Perceivable, Operable, Understandable, Robust) apply to terminal applications. This report adapts WCAG criteria to the CLI context, evaluating Cuervo CLI against applicable success criteria.

**Overall Compliance: Partial (42/100)**

| Principle | Score | Status |
|-----------|-------|--------|
| Perceivable | 35/100 | Fail |
| Operable | 55/100 | Partial |
| Understandable | 50/100 | Partial |
| Robust | 60/100 | Partial |

---

## 1. Perceivable

### 1.1 Text Alternatives (WCAG 1.1)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 1.1.1 Non-text Content | N/A | CLI is text-based; no images |

### 1.2 Time-based Media (WCAG 1.2)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 1.2.x All | N/A | No audio/video content |

### 1.3 Adaptable (WCAG 1.3)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 1.3.1 Info and Relationships | Fail | Doctor output uses Unicode box-drawing for structure. No semantic alternatives. Tool chrome uses visual `╭─`/`╰─` without text structure markers. |
| 1.3.2 Meaningful Sequence | Pass | Output follows logical reading order (top-to-bottom, left-to-right) |
| 1.3.3 Sensory Characteristics | Fail | Health status uses color as primary indicator (green=OK, yellow=warn, red=error). Doctor section headers rely on box-drawing position. |
| 1.3.4 Orientation | N/A | Terminal adjusts to window size |
| 1.3.5 Identify Input Purpose | Partial | API key prompt identified by text, but permission prompt `(y/n/a)` doesn't explain options |

**Remediation:**
1. Add text labels alongside all color indicators: `[OK]`, `[WARN]`, `[ERROR]`
2. Replace box-drawing with semantic text structures (headers, indentation)
3. Expand permission prompt to explain each option

### 1.4 Distinguishable (WCAG 1.4)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 1.4.1 Use of Color | Fail | Health indicators in doctor rely on color alone. Error/warning distinction is color-only in some contexts. |
| 1.4.3 Contrast (Minimum) | Unknown | ANSI colors render differently per terminal theme. No contrast guarantee for dark gray (color.muted) on dark backgrounds. |
| 1.4.4 Resize Text | N/A | Terminal font size controlled by user |
| 1.4.10 Reflow | Partial | Some output wraps properly but doctor box-drawing breaks on narrow terminals |
| 1.4.11 Non-text Contrast | Fail | Box-drawing characters and `╭─`/`╰─` may not have sufficient contrast |
| 1.4.12 Text Spacing | N/A | Monospace font, user controls |
| 1.4.13 Content on Hover | N/A | No hover interactions in terminal |

**Remediation:**
1. Never use color as sole indicator — always pair with text label
2. Test all color combinations against common terminal themes (Solarized, Dracula, Nord, default)
3. Support `NO_COLOR` environment variable
4. Replace box-drawing with ASCII alternatives when `TERM=dumb`

---

## 2. Operable

### 2.1 Keyboard Accessible (WCAG 2.1)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 2.1.1 Keyboard | Pass | All functionality accessible via keyboard (terminal is keyboard-first) |
| 2.1.2 No Keyboard Trap | Pass | Ctrl+C and Ctrl+D always available to exit |
| 2.1.4 Character Key Shortcuts | Pass | No single-character shortcuts that can't be disabled |

### 2.2 Enough Time (WCAG 2.2)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 2.2.1 Timing Adjustable | Partial | Permission prompt has no timeout (good), but model inference timeout is configurable only via config file |
| 2.2.2 Pause, Stop, Hide | Fail | No way to pause streaming output. Ctrl+C cancels entirely rather than pausing. |

**Remediation:**
1. Add Ctrl+S / Ctrl+Q for pause/resume of streaming output
2. Make timeout values visible and adjustable via CLI flags

### 2.3 Seizures and Physical Reactions (WCAG 2.3)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 2.3.1 Three Flashes | Pass | No flashing content. Spinner updates are smooth, not flashing. |

### 2.4 Navigable (WCAG 2.4)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 2.4.1 Bypass Blocks | Partial | `/clear` clears screen but no way to skip to specific section in long output |
| 2.4.2 Page Titled | Pass | REPL prompt clearly identifies application (`cuervo [model]`) |
| 2.4.3 Focus Order | Pass | Input always at prompt; tab order is N/A for CLI |
| 2.4.4 Link Purpose | N/A | No hyperlinks (terminal) |
| 2.4.6 Headings and Labels | Fail | Doctor sections use decorative box chars instead of clear text headings |
| 2.4.7 Focus Visible | Pass | Cursor always visible at prompt |
| 2.4.11 Focus Not Obscured | Pass | Prompt always at bottom of terminal |

**Remediation:**
1. Replace doctor box-drawing with clear text section headers
2. Add `--section` flag to doctor for jumping to specific section

### 2.5 Input Modalities (WCAG 2.5)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 2.5.x All | N/A | Terminal input is keyboard-only (no pointer, gesture, motion) |

---

## 3. Understandable

### 3.1 Readable (WCAG 3.1)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 3.1.1 Language of Page | N/A | Terminal doesn't declare language |
| 3.1.2 Language of Parts | N/A | Single-language application |
| 3.1.3 Unusual Words | Fail | Technical jargon used without explanation: "provider", "TBAC", "circuit breaker", "backpressure", "invocation metrics" |
| 3.1.4 Abbreviations | Partial | Model names abbreviated (sonnet, opus) with no expansion option |

**Remediation:**
1. Add glossary to /help or `cuervo help glossary`
2. Use plain language in user-facing output (e.g., "API service" not "provider")

### 3.2 Predictable (WCAG 3.2)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 3.2.1 On Focus | Pass | Typing in prompt doesn't trigger unexpected actions |
| 3.2.2 On Input | Pass | Enter sends query, no other automatic submissions |
| 3.2.3 Consistent Navigation | Fail | /help in REPL shows different commands than `cuervo --help` at CLI level |
| 3.2.4 Consistent Identification | Fail | Same concepts have different labels: "session" vs "conversation", error prefix inconsistency |

**Remediation:**
1. Align /help and --help content
2. Standardize terminology across all outputs

### 3.3 Input Assistance (WCAG 3.3)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 3.3.1 Error Identification | Partial | Errors identified but not always with actionable recovery |
| 3.3.2 Labels or Instructions | Partial | Permission prompt `(y/n/a)` doesn't explain what `a` means |
| 3.3.3 Error Suggestion | Fail | Many error messages lack recovery suggestions (see Heuristic Evaluation H9) |
| 3.3.4 Error Prevention | Fail | Destructive operations (memory prune) have no confirmation |
| 3.3.7 Redundant Entry | Pass | No redundant information requests |
| 3.3.8 Accessible Authentication | Pass | API key entry is straightforward text input |

**Remediation:**
1. Every error must include recovery suggestion
2. Destructive operations require confirmation
3. Permission prompt explains all options: `[y]es once  [n]o  [a]lways  [?]explain`

---

## 4. Robust

### 4.1 Compatible (WCAG 4.1)

| Criterion | Status | Finding |
|-----------|--------|---------|
| 4.1.1 Parsing | N/A (deprecated in 2.2) | — |
| 4.1.2 Name, Role, Value | Partial | Output has no semantic markup for screen readers. Terminal emulators rely on text structure. |
| 4.1.3 Status Messages | Fail | Session save, cache operations, and context loading produce no status messages. Spinner goes to stderr but no announcement mechanism. |

**Remediation:**
1. Ensure all status changes produce text output (even if brief)
2. Structure output with consistent patterns parseable by screen readers
3. Support `--json` for machine-readable output

---

## 5. CLI-Specific Accessibility Criteria

Beyond WCAG, these criteria apply specifically to terminal applications:

### 5.1 Terminal Compatibility

| Criterion | Status | Finding |
|-----------|--------|---------|
| Works with screen readers (VoiceOver, NVDA) | Unknown | Not tested. Unicode box characters may not read well. |
| Works with `TERM=dumb` | Fail | Box drawing and ANSI codes sent regardless of terminal capability |
| Works in `screen` / `tmux` | Pass | Standard ANSI escape sequences work in multiplexers |
| Works with terminal themes | Unknown | Dark gray on dark backgrounds may be invisible |

### 5.2 Machine Readability

| Criterion | Status | Finding |
|-----------|--------|---------|
| `NO_COLOR` support | Fail | Not implemented |
| `--json` output | Fail | Not implemented for any command |
| Structured exit codes | Fail | Not documented; most errors exit with code 1 |
| Piping stdout | Partial | Model output goes to stdout; chrome goes to stderr |

### 5.3 Cognitive Accessibility

| Criterion | Status | Finding |
|-----------|--------|---------|
| Consistent terminology | Fail | Multiple terms for same concepts |
| Error messages use plain language | Partial | Technical jargon in some error messages |
| Onboarding for new users | Fail | No guided setup or tutorial |
| Documentation available in-app | Partial | /help exists but is incomplete |

---

## 6. Compliance Roadmap

### Phase 1: Critical Fixes (WCAG A violations)

| Issue | WCAG | Fix | Effort |
|-------|------|-----|--------|
| Color as sole indicator | 1.4.1 | Add text labels to all color indicators | Small |
| `NO_COLOR` support | 1.4.1 | Implement env var check | Small |
| Error recovery suggestions | 3.3.3 | Add fix commands to all errors | Medium |
| Confirmation for destructive ops | 3.3.4 | Add prune confirmation | Small |

### Phase 2: AA Compliance

| Issue | WCAG | Fix | Effort |
|-------|------|-----|--------|
| Box-drawing fallback | 1.3.1 | ASCII alternatives for TERM=dumb | Small |
| Consistent navigation | 3.2.3 | Align /help and --help | Small |
| Consistent terminology | 3.2.4 | Audit and standardize | Medium |
| Status messages | 4.1.3 | Add text for all state changes | Medium |
| Permission explanation | 3.3.2 | Expand prompt options | Small |

### Phase 3: AAA Aspirations

| Issue | WCAG | Fix | Effort |
|-------|------|-----|--------|
| Glossary | 3.1.3 | Add technical term definitions | Small |
| Pause/resume streaming | 2.2.2 | Ctrl+S/Q support | Medium |
| Screen reader testing | 4.1.2 | Test with VoiceOver/NVDA | Medium |
| Full JSON output | 4.1.2 | --json for all commands | Large |

---

## 7. Testing Checklist

### For Every PR:

- [ ] No new color-only indicators (always pair with text label)
- [ ] Error messages include recovery suggestion
- [ ] Destructive operations have confirmation or --yes flag
- [ ] New output works with `NO_COLOR=1`
- [ ] New output works with terminal width < 40
- [ ] Terminology matches design system glossary

### Quarterly Accessibility Audit:

- [ ] Test with VoiceOver (macOS)
- [ ] Test with `TERM=dumb` in all commands
- [ ] Test with 5 popular terminal themes (contrast check)
- [ ] Review all new error messages for plain language
- [ ] Verify exit codes documented and correct

---

*WCAG compliance is an ongoing commitment. This report should be updated with each major release.*
