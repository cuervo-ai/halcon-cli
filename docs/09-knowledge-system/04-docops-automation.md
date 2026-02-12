# Fase 4 — Automatización Doc ↔ Code (DocOps)

> **Documento**: `09-knowledge-system/04-docops-automation.md`
> **Versión**: 1.0.0
> **Fecha**: 2026-02-06
> **Autores**: Technical Writer Automation Specialist, LLMOps Engineer, Software Architect
> **Estado**: Design Complete

---

## Índice

1. [Visión DocOps](#1-visión-docops)
2. [Generación desde Código (AST/TS/Swagger)](#2-generación-desde-código)
3. [Sincronización Automática](#3-sincronización-automática)
4. [Git Hooks](#4-git-hooks)
5. [Pipeline CI/CD](#5-pipeline-cicd)
6. [Coverage de Documentación](#6-coverage-de-documentación)
7. [Tests de Documentación](#7-tests-de-documentación)
8. [Score de Calidad Documental](#8-score-de-calidad-documental)
9. [Políticas y Reporting](#9-políticas-y-reporting)

---

## 1. Visión DocOps

### 1.1 Definición

**DocOps** = Documentation + DevOps. Aplica principios de CI/CD a la documentación técnica:

- **Docs-as-Code**: La documentación se trata como código — versionada, testeada, revisada
- **Continuous Documentation**: Cada cambio de código trigger verificación/actualización de docs
- **Quality Gates**: PRs no se mergean sin docs coverage mínimo
- **Automated Generation**: Código → Documentación → Validación → Publicación

### 1.2 Principios

| Principio | Implementación |
|-----------|---------------|
| **Documentación obligatoria por feature** | Gate en CI: PR con label "feature" requiere doc changes |
| **Freshness enforced** | Alerta automática si doc no se actualiza en 30 días tras code change |
| **Single source of truth** | API docs generadas desde OpenAPI spec, no escritas manualmente |
| **Test everything** | Links, examples, commands, schema compliance — todo testeado en CI |
| **Measure quality** | Doc Quality Score (DQS) computado por CI, visible en dashboards |

### 1.3 Flujo End-to-End

```
┌─────────────────────────────────────────────────────────────────────┐
│                      DOCOPS PIPELINE                                │
│                                                                     │
│  Developer writes code                                              │
│       │                                                             │
│       ▼                                                             │
│  git commit (pre-commit hook validates doc structure)               │
│       │                                                             │
│       ▼                                                             │
│  git push → triggers CI pipeline                                    │
│       │                                                             │
│       ├──▶ [Doc Quality Gate]                                       │
│       │     ├── Lint docs (markdownlint + Vale)                    │
│       │     ├── Check links                                         │
│       │     ├── Validate code examples                              │
│       │     ├── Compute doc coverage                                │
│       │     └── Compute DQS                                         │
│       │                                                             │
│       ├──▶ [Auto-Generation]                                        │
│       │     ├── Generate API docs from OpenAPI                      │
│       │     ├── Generate TypeDoc from TSDoc comments                │
│       │     ├── Update diagrams from code structure                 │
│       │     └── Update changelog from conventional commits          │
│       │                                                             │
│       ├──▶ [Freshness Check]                                        │
│       │     ├── Compare doc modified dates vs code modified dates   │
│       │     ├── Flag stale docs                                     │
│       │     └── Trigger DocAgent for stale docs (batch mode)        │
│       │                                                             │
│       └──▶ [Knowledge Indexing]                                     │
│             ├── Re-index changed docs into knowledge store          │
│             ├── Update graph relations                              │
│             └── Invalidate semantic cache                           │
│                                                                     │
│  All gates pass?                                                    │
│       │                                                             │
│       ├── YES → PR mergeable                                        │
│       └── NO → Block merge + annotate PR with issues                │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 2. Generación desde Código

### 2.1 Fuentes de Generación Automática

| Fuente | Output | Herramienta | Trigger |
|--------|--------|-------------|---------|
| OpenAPI/Swagger spec | API reference docs | Redocly / custom generator | Spec file change |
| TSDoc comments | TypeDoc HTML/MD | TypeDoc + typedoc-plugin-markdown | Source file change |
| Rust doc comments | rustdoc | cargo doc | Source file change |
| AST (Tree-sitter) | Module/class summaries | Custom via treesitter.rs | Source file change |
| Conventional Commits | CHANGELOG.md | conventional-changelog | git push to main |
| Package.json/Cargo.toml | Dependency docs | Custom parser | Dependency change |
| Mermaid source | Rendered diagrams (SVG/PNG) | mermaid-cli (mmdc) | .mmd file change |
| Database migrations | Schema docs | Custom parser | Migration file change |

### 2.2 API Documentation Pipeline

```
┌────────────────────────────────────────────────────────┐
│          API DOC GENERATION PIPELINE                    │
│                                                         │
│  Source: OpenAPI 3.1 spec (YAML)                       │
│  Location: docs/api/openapi.yaml                       │
│       │                                                 │
│       ▼                                                 │
│  ┌──────────────────┐                                  │
│  │  Spec Validation  │                                  │
│  │  (spectral lint)  │                                  │
│  └────────┬─────────┘                                  │
│           │                                             │
│       ┌───┴───┐                                        │
│       │       │                                        │
│       ▼       ▼                                        │
│  ┌────────┐ ┌────────────┐                             │
│  │ Redocly│ │ Custom MD  │                             │
│  │ HTML   │ │ Generator  │                             │
│  │ (site) │ │ (per-endpoint│                           │
│  │        │ │  markdown)  │                             │
│  └────────┘ └────────────┘                             │
│       │            │                                    │
│       ▼            ▼                                    │
│  docs/api/     docs/api/endpoints/                     │
│  index.html    POST-auth-login.md                      │
│                GET-users-{id}.md                        │
│                ...                                      │
└────────────────────────────────────────────────────────┘
```

### 2.3 TypeDoc Integration

```typescript
// typedoc.config.ts
const config: TypeDocOptions = {
  entryPoints: ['src/index.ts'],
  out: 'docs/api/typescript',
  plugin: ['typedoc-plugin-markdown'],
  outputFileStrategy: 'modules',
  readme: 'none',
  excludePrivate: true,
  excludeInternal: true,
  categorizeByGroup: true,
  navigationModel: {
    excludeGroups: true,
  },
  // Asegurar que TSDoc comments generen docs útiles
  validation: {
    notExported: true,
    invalidLink: true,
    notDocumented: false,            // Warning, not error (covered by coverage check)
  },
};
```

### 2.4 AST-Based Documentation Generation

```typescript
interface ASTDocGenerator {
  /**
   * Given a source file, extract public API surface and generate
   * documentation skeleton. Used by DocAgent as starting point.
   */
  generateSkeleton(
    filePath: string,
    options: GenerationOptions
  ): Promise<DocSkeleton>;
}

interface DocSkeleton {
  module: string;
  description: string;                // From file-level JSDoc or inferred
  exports: ExportDoc[];
}

interface ExportDoc {
  name: string;
  type: 'function' | 'class' | 'interface' | 'type' | 'const' | 'enum';
  signature: string;
  jsdoc: string | null;              // Existing JSDoc if present
  parameters?: ParameterDoc[];
  returnType?: string;
  examples?: string[];               // From @example tags
  needsDocumentation: boolean;       // True if no JSDoc exists
}

// Ejemplo de output:
// {
//   module: "src/infrastructure/auth/auth.service.ts",
//   description: "Authentication service handling JWT and OAuth2 flows",
//   exports: [
//     {
//       name: "AuthService",
//       type: "class",
//       signature: "class AuthService implements IAuthService",
//       jsdoc: null,
//       needsDocumentation: true
//     },
//     {
//       name: "validateCredentials",
//       type: "function",
//       signature: "async function validateCredentials(creds: UserCredentials, provider: AuthProvider): Promise<AuthToken>",
//       jsdoc: "/** Validates user credentials against the auth provider... */",
//       parameters: [
//         { name: "creds", type: "UserCredentials", description: "..." },
//         { name: "provider", type: "AuthProvider", description: "..." }
//       ],
//       returnType: "Promise<AuthToken>",
//       needsDocumentation: false
//     }
//   ]
// }
```

---

## 3. Sincronización Automática

### 3.1 Change Detection Matrix

Define qué cambios de código requieren qué actualización de documentación:

```typescript
const syncRules: SyncRule[] = [
  {
    // Cambio en API endpoint → actualizar API docs
    codePattern: 'src/**/controllers/**/*.ts',
    affectedDocs: ['docs/api/endpoints/**/*.md'],
    action: 'validate_and_flag',
    severity: 'error',               // Block merge if not updated
  },
  {
    // Cambio en interfaces públicas → actualizar TypeDoc
    codePattern: 'src/domain/**/*.ts',
    affectedDocs: ['docs/api/typescript/**/*.md'],
    action: 'auto_regenerate',
    severity: 'warning',
  },
  {
    // Cambio en configuración → actualizar config docs
    codePattern: 'config/**/*',
    affectedDocs: ['docs/**/configuration*.md', 'docs/**/config*.md'],
    action: 'validate_and_flag',
    severity: 'warning',
  },
  {
    // Nuevo ADR necesario si se cambia arquitectura
    codePattern: 'src/infrastructure/**/*.ts',
    affectedDocs: ['docs/03-architecture/**/*.md'],
    action: 'suggest_adr',
    severity: 'info',
  },
  {
    // Cambio en dependencias → actualizar setup docs
    codePattern: 'package.json',
    affectedDocs: ['README.md', 'docs/**/setup*.md', 'docs/**/installation*.md'],
    action: 'validate_and_flag',
    severity: 'warning',
  },
  {
    // Cambio en schema DB → actualizar data model docs
    codePattern: 'src/infrastructure/storage/migrations/**/*',
    affectedDocs: ['docs/**/data-model*.md', 'docs/**/schema*.md'],
    action: 'validate_and_flag',
    severity: 'error',
  },
];

interface SyncRule {
  codePattern: string;                // Glob pattern for source code
  affectedDocs: string[];             // Glob patterns for affected docs
  action: 'auto_regenerate' | 'validate_and_flag' | 'suggest_adr' | 'trigger_agent';
  severity: 'error' | 'warning' | 'info';
}
```

### 3.2 Staleness Detection

```typescript
interface StalenessDetector {
  /**
   * For each documentation file, determine if it's stale by comparing
   * its last modification date with the modification dates of the
   * code it documents.
   */
  detectStale(repository: string): Promise<StalenessReport>;
}

interface StalenessReport {
  totalDocs: number;
  freshDocs: DocFreshness[];
  staleDocs: DocFreshness[];
  criticallyStale: DocFreshness[];    // >90 days since related code change
}

interface DocFreshness {
  docPath: string;
  docLastModified: Date;
  relatedCodePaths: string[];
  codeLastModified: Date;             // Most recent change in related code
  stalenessDays: number;              // Days between code change and doc update
  staleSince: Date | null;            // When it became stale
  severity: 'fresh' | 'aging' | 'stale' | 'critically_stale';
}

// Thresholds:
// fresh: doc updated after or within 7 days of code change
// aging: 7-30 days since code change without doc update
// stale: 30-90 days
// critically_stale: >90 days
```

---

## 4. Git Hooks

### 4.1 Pre-commit Hook

```bash
#!/bin/bash
# .husky/pre-commit

# 1. Lint markdown files that are staged
STAGED_MD=$(git diff --cached --name-only --diff-filter=ACM | grep '\.md$')

if [ -n "$STAGED_MD" ]; then
  echo "🔍 Validating documentation..."

  # Markdown lint
  echo "$STAGED_MD" | xargs npx markdownlint-cli2
  if [ $? -ne 0 ]; then
    echo "❌ Markdown lint failed. Fix issues before committing."
    exit 1
  fi

  # Check for broken internal links in staged files
  echo "$STAGED_MD" | xargs npx cuervo-doc-links --check
  if [ $? -ne 0 ]; then
    echo "❌ Broken links detected. Fix before committing."
    exit 1
  fi

  echo "✅ Documentation validation passed"
fi
```

### 4.2 Commit-msg Hook

```bash
#!/bin/bash
# .husky/commit-msg

# Validate conventional commits format
npx commitlint --edit "$1"

# Check: if code files changed, warn if no doc files changed
CODE_CHANGED=$(git diff --cached --name-only --diff-filter=ACM | grep -c '^src/')
DOC_CHANGED=$(git diff --cached --name-only --diff-filter=ACM | grep -c '^docs/')

if [ "$CODE_CHANGED" -gt 0 ] && [ "$DOC_CHANGED" -eq 0 ]; then
  echo "⚠️  Code changed without documentation updates."
  echo "   Consider updating docs if this change affects public APIs."
  # Warning only, not blocking
fi
```

### 4.3 Post-merge Hook (Knowledge Index Trigger)

```bash
#!/bin/bash
# .husky/post-merge

# Trigger incremental knowledge store re-indexing
CHANGED_FILES=$(git diff --name-only HEAD@{1} HEAD)

if echo "$CHANGED_FILES" | grep -qE '\.(md|ts|rs|yaml|json)$'; then
  echo "📚 Triggering knowledge store re-indexing..."

  # Async: don't block the developer
  npx cuervo-knowledge reindex \
    --changed-files <(echo "$CHANGED_FILES") \
    --async \
    --notify-on-complete &

  echo "   Re-indexing started in background."
fi
```

---

## 5. Pipeline CI/CD

### 5.1 GitHub Actions Workflow

```yaml
# .github/workflows/docops.yml
name: DocOps Pipeline

on:
  pull_request:
    types: [opened, synchronize]
  push:
    branches: [main]

permissions:
  contents: read
  pull-requests: write
  checks: write

jobs:
  # ─── STAGE 1: Validation (runs on every PR) ──────────
  doc-validation:
    name: "📋 Doc Validation"
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '22'

      - name: Install dependencies
        run: npm ci

      - name: Markdown Lint
        run: npx markdownlint-cli2 "docs/**/*.md"

      - name: Prose Lint (Vale)
        uses: errata-ai/vale-action@v2
        with:
          files: docs/
          config: .vale.ini
          reporter: github-pr-review

      - name: Check Links
        run: npx cuervo-doc-links --check --base-dir docs/

      - name: Validate Code Examples
        run: npx cuervo-doc-examples --validate --base-dir docs/

      - name: Validate Mermaid Diagrams
        run: npx @mermaid-js/mermaid-cli -i docs/diagrams/*.mmd --validate

  # ─── STAGE 2: Coverage Analysis ───────────────────────
  doc-coverage:
    name: "📊 Doc Coverage"
    runs-on: ubuntu-latest
    needs: doc-validation
    steps:
      - uses: actions/checkout@v4

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '22'

      - name: Install dependencies
        run: npm ci

      - name: Compute Documentation Coverage
        id: coverage
        run: |
          npx cuervo-doc-coverage \
            --source "src/**/*.ts" \
            --docs "docs/**/*.md" \
            --output coverage-report.json \
            --format json

      - name: Check Coverage Threshold
        run: |
          COVERAGE=$(jq '.overallCoverage' coverage-report.json)
          echo "Documentation coverage: ${COVERAGE}%"
          if (( $(echo "$COVERAGE < 70" | bc -l) )); then
            echo "❌ Documentation coverage below 70% threshold"
            exit 1
          fi

      - name: Post Coverage Report to PR
        if: github.event_name == 'pull_request'
        uses: actions/github-script@v7
        with:
          script: |
            const report = require('./coverage-report.json');
            const body = `## 📊 Documentation Coverage Report

            | Metric | Value | Target |
            |--------|-------|--------|
            | Overall Coverage | ${report.overallCoverage}% | ≥70% |
            | Public APIs | ${report.apiCoverage}% | ≥80% |
            | New Symbols | ${report.newSymbolsCoverage}% | 100% |
            | Stale Docs | ${report.staleDocs} | 0 |

            ${report.undocumented.length > 0 ?
              '### Undocumented Symbols\n' +
              report.undocumented.map(s => `- \`${s.name}\` (${s.file}:${s.line})`).join('\n')
              : '✅ All public symbols documented'}
            `;
            github.rest.issues.createComment({
              issue_number: context.issue.number,
              owner: context.repo.owner,
              repo: context.repo.repo,
              body
            });

  # ─── STAGE 3: Quality Score ───────────────────────────
  doc-quality:
    name: "⭐ Doc Quality Score"
    runs-on: ubuntu-latest
    needs: doc-validation
    steps:
      - uses: actions/checkout@v4

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '22'

      - name: Install dependencies
        run: npm ci

      - name: Compute Doc Quality Score (DQS)
        run: |
          npx cuervo-doc-quality \
            --docs "docs/**/*.md" \
            --output dqs-report.json

      - name: Quality Gate
        run: |
          AVG_DQS=$(jq '.averageDQS' dqs-report.json)
          echo "Average DQS: ${AVG_DQS}/100"
          if (( $(echo "$AVG_DQS < 60" | bc -l) )); then
            echo "❌ Documentation quality below minimum threshold (60/100)"
            exit 1
          fi

  # ─── STAGE 4: Freshness Check ────────────────────────
  doc-freshness:
    name: "🕐 Freshness Check"
    runs-on: ubuntu-latest
    needs: doc-validation
    if: github.event_name == 'pull_request'
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0              # Full history for git log analysis

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '22'

      - name: Install dependencies
        run: npm ci

      - name: Check Code→Doc Sync
        run: |
          npx cuervo-doc-freshness \
            --changed-files "$(git diff --name-only origin/main...HEAD)" \
            --sync-rules .docops/sync-rules.yaml \
            --output freshness-report.json

      - name: Enforce Documentation for Features
        if: contains(github.event.pull_request.labels.*.name, 'feature')
        run: |
          DOC_CHANGED=$(git diff --name-only origin/main...HEAD | grep -c '^docs/' || true)
          if [ "$DOC_CHANGED" -eq 0 ]; then
            echo "❌ Feature PRs require documentation changes."
            echo "   Add or update documentation in the docs/ directory."
            exit 1
          fi

  # ─── STAGE 5: Knowledge Re-indexing (main only) ──────
  knowledge-index:
    name: "🧠 Knowledge Index"
    runs-on: ubuntu-latest
    needs: [doc-validation, doc-coverage, doc-quality]
    if: github.ref == 'refs/heads/main'
    steps:
      - uses: actions/checkout@v4

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '22'

      - name: Install dependencies
        run: npm ci

      - name: Re-index Knowledge Store
        env:
          KNOWLEDGE_DB_URL: ${{ secrets.KNOWLEDGE_DB_URL }}
          VOYAGE_API_KEY: ${{ secrets.VOYAGE_API_KEY }}
          COHERE_API_KEY: ${{ secrets.COHERE_API_KEY }}
        run: |
          npx cuervo-knowledge reindex \
            --changed-files "$(git diff --name-only HEAD~1..HEAD)" \
            --commit-sha "${{ github.sha }}"

  # ─── STAGE 6: Auto-generation (main only) ────────────
  doc-generation:
    name: "🤖 Auto-generation"
    runs-on: ubuntu-latest
    needs: knowledge-index
    if: github.ref == 'refs/heads/main'
    steps:
      - uses: actions/checkout@v4

      - name: Setup Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '22'

      - name: Install dependencies
        run: npm ci

      - name: Generate API Docs
        run: npx cuervo-doc-gen api --source "src/**/*.ts" --output docs/api/

      - name: Generate TypeDoc
        run: npx typedoc

      - name: Update Changelog
        run: npx conventional-changelog -p angular -i CHANGELOG.md -s

      - name: Create PR if changes
        run: |
          if [ -n "$(git status --porcelain docs/)" ]; then
            git checkout -b docs/auto-update-${{ github.sha }}
            git add docs/
            git commit -m "docs: auto-update generated documentation

            Triggered by commit ${{ github.sha }}

            Co-Authored-By: DocAgent <docagent@cuervo.dev>"
            gh pr create \
              --title "docs: auto-update from $(date +%Y-%m-%d)" \
              --body "Automated documentation update triggered by code changes." \
              --label "documentation,automated"
          fi
```

### 5.2 Vale Configuration

```ini
# .vale.ini
StylesPath = .vale/styles

MinAlertLevel = suggestion

Packages = Microsoft, write-good, Readability

[docs/*.md]
BasedOnStyles = Vale, Microsoft, write-good, Readability, Cuervo

# Custom Cuervo rules
[docs/*.md]
Vale.Terms = YES
Microsoft.Headings = YES
Microsoft.Passive = suggestion
write-good.Weasel = warning
write-good.TooWordy = suggestion
Readability.FleschKincaid = suggestion

# Don't lint code blocks
BlockIgnores = (?s)(```.*?```)
TokenIgnores = (\$\{.*?\})
```

---

## 6. Coverage de Documentación

### 6.1 Métricas de Coverage

```typescript
interface DocCoverageMetrics {
  // ─── API COVERAGE ─────────────────────────────
  api: {
    totalPublicExports: number;
    documentedExports: number;
    coveragePercent: number;
    byType: {
      functions: { total: number; documented: number };
      classes: { total: number; documented: number };
      interfaces: { total: number; documented: number };
      types: { total: number; documented: number };
      enums: { total: number; documented: number };
    };
    undocumented: {
      name: string;
      type: string;
      file: string;
      line: number;
      complexity: number;           // Higher complexity → higher priority to document
    }[];
  };

  // ─── CONCEPTUAL COVERAGE ──────────────────────
  conceptual: {
    totalFeatures: number;          // From requirements (RF-xxx)
    documentedFeatures: number;
    coveragePercent: number;
    undocumented: string[];
  };

  // ─── ARCHITECTURE COVERAGE ────────────────────
  architecture: {
    totalADRs: number;
    totalModules: number;
    modulesWithDocs: number;
    coveragePercent: number;
    suggestedADRs: string[];
  };

  // ─── ENDPOINT COVERAGE ────────────────────────
  endpoints: {
    totalEndpoints: number;
    documentedEndpoints: number;
    coveragePercent: number;
    undocumented: {
      method: string;
      path: string;
      controller: string;
    }[];
  };
}
```

### 6.2 Coverage Computation Algorithm

```typescript
async function computeCoverage(repository: string): Promise<DocCoverageMetrics> {
  // 1. Extract all public symbols from code
  const symbols = await extractPublicSymbols(repository);

  // 2. For each symbol, check if documentation exists
  for (const symbol of symbols) {
    // Check 1: Does the symbol have TSDoc/JSDoc comments?
    symbol.hasInlineDoc = symbol.jsdoc !== null && symbol.jsdoc.length > 20;

    // Check 2: Is the symbol referenced in any markdown documentation?
    const searchResults = await knowledgeStore.search({
      text: symbol.name,
      filters: {
        contentTypes: ['prose', 'api_endpoint'],
        repositories: [repository],
      },
      topK: 3,
    });
    symbol.hasExternalDoc = searchResults.results.some(
      r => r.score > 0.8 && r.chunk.content.includes(symbol.name)
    );

    // Symbol is "documented" if it has inline doc OR external doc
    symbol.isDocumented = symbol.hasInlineDoc || symbol.hasExternalDoc;
  }

  // 3. Compute metrics
  return {
    api: {
      totalPublicExports: symbols.length,
      documentedExports: symbols.filter(s => s.isDocumented).length,
      coveragePercent: (symbols.filter(s => s.isDocumented).length / symbols.length) * 100,
      // ...
    },
    // ...
  };
}
```

---

## 7. Tests de Documentación

### 7.1 Test Categories

```typescript
interface DocTestSuite {
  // ─── STRUCTURAL TESTS ──────────────────────────
  structural: {
    // Markdown válido (parseable)
    validMarkdown: TestCase[];

    // Heading hierarchy correct (H1 → H2 → H3, no skipping)
    headingHierarchy: TestCase[];

    // Required sections present (per template)
    requiredSections: TestCase[];

    // No duplicate headings within a file
    noDuplicateHeadings: TestCase[];
  };

  // ─── LINK TESTS ────────────────────────────────
  links: {
    // All internal links resolve to existing files
    internalLinks: TestCase[];

    // All external URLs return 2xx
    externalLinks: TestCase[];

    // All anchor links (#section) resolve
    anchorLinks: TestCase[];

    // No orphaned documents (not linked from anywhere)
    noOrphans: TestCase[];
  };

  // ─── CODE EXAMPLE TESTS ────────────────────────
  codeExamples: {
    // TypeScript examples compile (tsc --noEmit)
    tsCompiles: TestCase[];

    // Shell examples are syntactically valid
    shellValid: TestCase[];

    // JSON/YAML examples parse correctly
    dataFormats: TestCase[];

    // Import paths in examples exist
    importPaths: TestCase[];
  };

  // ─── CONTENT TESTS ─────────────────────────────
  content: {
    // No placeholder text (TODO, FIXME, TBD, Lorem ipsum)
    noPlaceholders: TestCase[];

    // No sensitive information (API keys, passwords, emails)
    noSecrets: TestCase[];

    // Version references are current
    versionsCurrent: TestCase[];

    // File paths mentioned in docs exist
    filePathsExist: TestCase[];
  };

  // ─── DIAGRAM TESTS ─────────────────────────────
  diagrams: {
    // Mermaid diagrams render without errors
    mermaidValid: TestCase[];

    // Diagrams have alt text / caption
    diagramsAccessible: TestCase[];
  };
}
```

### 7.2 Test Implementation

```typescript
// Link checker
async function checkInternalLinks(docsDir: string): Promise<TestResult[]> {
  const results: TestResult[] = [];
  const mdFiles = await glob(`${docsDir}/**/*.md`);

  for (const file of mdFiles) {
    const content = await readFile(file, 'utf-8');
    const links = extractMarkdownLinks(content);

    for (const link of links) {
      if (link.href.startsWith('http')) continue;    // External, skip
      if (link.href.startsWith('#')) {
        // Anchor link — check heading exists in same file
        const headings = extractHeadings(content);
        const anchorTarget = link.href.slice(1);
        const exists = headings.some(h => slugify(h) === anchorTarget);
        results.push({
          file,
          test: 'anchor_link',
          target: link.href,
          passed: exists,
          message: exists ? undefined : `Anchor "${link.href}" not found in ${file}`,
        });
      } else {
        // Relative file link
        const resolved = path.resolve(path.dirname(file), link.href);
        const exists = await fileExists(resolved);
        results.push({
          file,
          test: 'internal_link',
          target: link.href,
          passed: exists,
          message: exists ? undefined : `File not found: ${resolved}`,
        });
      }
    }
  }

  return results;
}

// Code example validator
async function validateCodeExamples(docsDir: string): Promise<TestResult[]> {
  const results: TestResult[] = [];
  const mdFiles = await glob(`${docsDir}/**/*.md`);

  for (const file of mdFiles) {
    const content = await readFile(file, 'utf-8');
    const codeBlocks = extractCodeBlocks(content);

    for (const block of codeBlocks) {
      if (block.language === 'typescript' || block.language === 'ts') {
        // Write to temp file and check with tsc
        const tempFile = writeTempFile(block.content, '.ts');
        const { exitCode, stderr } = await exec(`npx tsc --noEmit --strict ${tempFile}`);
        results.push({
          file,
          test: 'ts_compiles',
          line: block.line,
          passed: exitCode === 0,
          message: exitCode !== 0 ? `TypeScript error: ${stderr}` : undefined,
        });
      }

      if (block.language === 'json') {
        try {
          JSON.parse(block.content);
          results.push({ file, test: 'json_valid', line: block.line, passed: true });
        } catch (e) {
          results.push({
            file,
            test: 'json_valid',
            line: block.line,
            passed: false,
            message: `Invalid JSON: ${e.message}`,
          });
        }
      }

      if (block.language === 'yaml' || block.language === 'yml') {
        try {
          YAML.parse(block.content);
          results.push({ file, test: 'yaml_valid', line: block.line, passed: true });
        } catch (e) {
          results.push({
            file,
            test: 'yaml_valid',
            line: block.line,
            passed: false,
            message: `Invalid YAML: ${e.message}`,
          });
        }
      }
    }
  }

  return results;
}
```

---

## 8. Score de Calidad Documental

### 8.1 Doc Quality Score (DQS) — Cálculo Detallado

```typescript
interface DQSCalculator {
  compute(docPath: string): Promise<DQSReport>;
}

interface DQSReport {
  path: string;
  overall: number;                    // 0-100

  dimensions: {
    // Automated checks (60% of total)
    structure: {                       // 10%
      score: number;
      checks: {
        validMarkdown: boolean;
        correctHeadingHierarchy: boolean;
        hasTableOfContents: boolean;    // For docs > 500 lines
        consistentFormatting: boolean;
      };
    };

    links: {                           // 10%
      score: number;
      checks: {
        allInternalLinksValid: boolean;
        allExternalLinksValid: boolean;
        noOrphanedSections: boolean;
      };
    };

    codeExamples: {                    // 10%
      score: number;
      checks: {
        allExamplesCompile: boolean;
        examplesHaveLanguageTags: boolean;
        examplesAreRealistic: boolean;  // Not just "foo/bar"
      };
    };

    freshness: {                       // 15%
      score: number;
      daysSinceUpdate: number;
      daysSinceRelatedCodeChange: number;
      isStale: boolean;
    };

    coverage: {                        // 15%
      score: number;
      symbolsCovered: number;
      symbolsTotal: number;
    };

    // LLM-evaluated (40% of total — computed by DocAgent)
    accuracy: {                        // 15%
      score: number;
      discrepanciesFound: number;
      details: string;
    };

    completeness: {                    // 10%
      score: number;
      missingSections: string[];
      details: string;
    };

    clarity: {                         // 10%
      score: number;
      readabilityGrade: number;        // Flesch-Kincaid
      details: string;
    };

    consistency: {                     // 5%
      score: number;
      terminologyIssues: number;
      styleIssues: number;
    };
  };

  trend: {
    previousScore: number | null;
    direction: 'improving' | 'stable' | 'declining';
    history: { date: Date; score: number }[];
  };
}
```

### 8.2 Scoring Thresholds

| Score Range | Label | Action |
|------------|-------|--------|
| 90-100 | Excellent | No action needed |
| 75-89 | Good | Minor improvements suggested |
| 60-74 | Acceptable | Improvements recommended, tracked in backlog |
| 40-59 | Poor | DocAgent triggered to improve, PR created |
| 0-39 | Critical | Immediate attention required, blocks related PRs |

---

## 9. Políticas y Reporting

### 9.1 Documentation Policies

```yaml
# .docops/policies.yaml
policies:
  # Feature documentation is mandatory
  feature_requires_docs:
    enabled: true
    enforcement: block_merge
    applies_to:
      labels: ["feature", "enhancement"]
    rule: "PR must include changes in docs/ directory"

  # API changes require API doc updates
  api_change_requires_docs:
    enabled: true
    enforcement: block_merge
    applies_to:
      paths: ["src/**/controllers/**", "src/**/routes/**"]
    rule: "Changes to API controllers must update API documentation"

  # Minimum documentation coverage
  minimum_coverage:
    enabled: true
    enforcement: block_merge
    threshold: 70
    metric: "overallCoverage"

  # Minimum quality score
  minimum_quality:
    enabled: true
    enforcement: warning
    threshold: 60
    metric: "averageDQS"

  # Freshness policy
  max_staleness:
    enabled: true
    enforcement: warning
    max_days: 30
    notification: slack

  # Breaking changes require ADR
  breaking_change_requires_adr:
    enabled: true
    enforcement: block_merge
    applies_to:
      labels: ["breaking-change"]
    rule: "Breaking changes must include an ADR in docs/architecture/"
```

### 9.2 Weekly Report Template

```markdown
# 📊 Documentation Health Report — Week of {date}

## Summary
| Metric | Value | Target | Status |
|--------|-------|--------|--------|
| Overall DQS | {avg_dqs}/100 | ≥75 | {status_emoji} |
| Doc Coverage | {coverage}% | ≥80% | {status_emoji} |
| Stale Docs | {stale_count} | 0 | {status_emoji} |
| Broken Links | {broken_links} | 0 | {status_emoji} |
| Undocumented APIs | {undoc_count} | 0 | {status_emoji} |

## Quality Trend
{sparkline_or_table_of_weekly_scores}

## Top Issues
1. {issue_1}
2. {issue_2}
3. {issue_3}

## Agent Activity
- Tasks completed: {agent_tasks}
- Docs generated: {docs_generated}
- Docs updated: {docs_updated}
- PRs created: {prs_created}
- Success rate: {success_rate}%

## Recommendations
{agent_recommendations}
```

### 9.3 Dashboard Metrics (for Langfuse/Grafana)

```typescript
interface DocOpsDashboard {
  // Real-time metrics
  realtime: {
    currentDQS: number;
    currentCoverage: number;
    staleDocs: number;
    brokenLinks: number;
    pendingDocPRs: number;
    agentTasksInProgress: number;
  };

  // Historical metrics (for trend analysis)
  historical: {
    dqsTrend: TimeSeries;             // Weekly DQS over time
    coverageTrend: TimeSeries;        // Weekly coverage over time
    stalenessTrend: TimeSeries;       // Weekly stale docs count
    agentSuccessRate: TimeSeries;     // Weekly agent success rate
  };

  // Per-document breakdown
  perDocument: {
    path: string;
    dqs: number;
    freshness: number;
    lastUpdated: Date;
    lastReviewed: Date;
    owner: string;
  }[];
}
```

---

*Siguiente documento: [05-best-practices-2026.md](./05-best-practices-2026.md) — Mejores Prácticas y Estándares 2026*
