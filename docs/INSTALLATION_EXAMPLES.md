# 📸 Ejemplos Visuales de Instalación

Esta guía muestra exactamente lo que verás cuando instales Cuervo CLI.

---

## 🐧 Instalación en Linux / macOS

### Comando de Instalación

```bash
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
```

### Salida Esperada

```
   ╔═══════════════════════════════════════╗
   ║      Cuervo CLI - Installation        ║
   ╚═══════════════════════════════════════╝

━━━ Detecting platform ━━━

[INFO]  OS:           darwin
[INFO]  Architecture: aarch64
[INFO]  libc:
[✓]     Target:       aarch64-apple-darwin

━━━ Preparing download ━━━

[INFO]  Asset:    cuervo-aarch64-apple-darwin.tar.gz
[INFO]  URL:      https://github.com/cuervo-ai/cuervo-cli/releases/latest/download/cuervo-aarch64-apple-darwin.tar.gz

━━━ Downloading binary ━━━

[✓]     Downloaded cuervo-aarch64-apple-darwin.tar.gz

━━━ Verifying integrity ━━━

[✓]     Checksum verified

━━━ Extracting archive ━━━

[✓]     Extracted binary: cuervo

━━━ Installing ━━━

[✓]     Installed to /Users/username/.local/bin/cuervo

━━━ Configuring PATH ━━━

[INFO]  Adding /Users/username/.local/bin to PATH in /Users/username/.zshrc
[✓]     Added to PATH. Run: source /Users/username/.zshrc

━━━ Verification ━━━

[✓]     Installation verified: cuervo 0.1.0 (f8f41dd0, aarch64-apple-darwin)

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
   Installation complete! 🎉
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  Next steps:

  1. Reload your shell:
     source /Users/username/.zshrc

  2. Verify installation:
     cuervo --version

  3. Get started:
     cuervo --help

  Documentation: https://github.com/cuervo-ai/cuervo-cli
```

**Tiempo total:** ~8-12 segundos

---

## 🪟 Instalación en Windows

### Comando de Instalación

```powershell
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
```

### Salida Esperada

```
╔═══════════════════════════════════════╗
║      Cuervo CLI - Installation        ║
╚═══════════════════════════════════════╝

━━━ Detecting platform ━━━

[INFO]  Target: x86_64-pc-windows-msvc

━━━ Preparing download ━━━

[INFO]  Asset: cuervo-x86_64-pc-windows-msvc.zip
[INFO]  URL:   https://github.com/cuervo-ai/cuervo-cli/releases/latest/download/cuervo-x86_64-pc-windows-msvc.zip

━━━ Downloading binary ━━━

[✓]     Downloaded cuervo-x86_64-pc-windows-msvc.zip

━━━ Verifying integrity ━━━

[✓]     Checksum verified

━━━ Extracting archive ━━━

[✓]     Extracted binary: C:\Users\username\AppData\Local\Temp\cuervo-install-12345\cuervo.exe

━━━ Installing ━━━

[✓]     Installed to C:\Users\username\.local\bin\cuervo.exe

━━━ Configuring PATH ━━━

[INFO]  Adding C:\Users\username\.local\bin to PATH
[✓]     Added to PATH (restart terminal to apply)

━━━ Verification ━━━

[✓]     Installation verified: cuervo 0.1.0 (f8f41dd0, x86_64-pc-windows-msvc)

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
   Installation complete! 🎉
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

  Next steps:

  1. Restart your terminal

  2. Verify installation:
     cuervo --version

  3. Get started:
     cuervo --help

  Documentation: https://github.com/cuervo-ai/cuervo-cli
```

**Tiempo total:** ~10-15 segundos

---

## ✅ Verificación Post-Instalación

### Comando

```bash
cuervo --version
```

### Salida

```
cuervo 0.1.0 (f8f41dd0 2026-02-14, aarch64-apple-darwin)
```

### Comando de Diagnóstico

```bash
cuervo doctor
```

### Salida Esperada

```
🔍 Cuervo CLI - System Diagnostics

━━━ Environment ━━━
  ✓ Binary:       /Users/username/.local/bin/cuervo
  ✓ Version:      0.1.0 (f8f41dd0)
  ✓ Platform:     aarch64-apple-darwin
  ✓ Rust version: 1.90.0

━━━ Configuration ━━━
  ✓ Config file:  /Users/username/.cuervo/config.toml
  ✓ Database:     /Users/username/.cuervo/cuervo.db
  ✓ Permissions:  OK

━━━ Providers ━━━
  ✓ Anthropic:    Configured (claude-sonnet-4-5-20250929)
  ✓ OpenAI:       Configured (gpt-4o-mini)
  ✓ DeepSeek:     Configured (deepseek-chat)
  ✓ Ollama:       Running (http://localhost:11434)

━━━ Tools ━━━
  ✓ Total tools:  12 registered
  ✓ Sandbox:      Enabled
  ✓ Dry-run:      Disabled
  ✓ Confirm:      Enabled (destructive actions)

━━━ Security ━━━
  ✓ PII Detection:  Enabled
  ✓ Audit Log:      Enabled
  ✓ Keyring:        Available

━━━ MCP Servers ━━━
  ✓ filesystem:   Running
  ✓ git:          Running
  ⚠ custom-tool:  Not configured

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  All systems operational ✓
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Warnings:
  - MCP server 'custom-tool' not configured (optional)

Recommendations:
  - Configure API keys: cuervo auth login <provider>
  - Review config: cuervo config show
```

---

## 🔄 Actualización

### Comando

```bash
# Re-ejecutar el instalador
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
```

### Salida (cuando ya está instalado)

```
   ╔═══════════════════════════════════════╗
   ║      Cuervo CLI - Installation        ║
   ╚═══════════════════════════════════════╝

━━━ Detecting platform ━━━

[✓]     Target:       aarch64-apple-darwin

━━━ Preparing download ━━━

[INFO]  Downloading latest version...

━━━ Installing ━━━

[INFO]  Replacing existing installation at /Users/username/.local/bin/cuervo
[✓]     Updated to version 0.2.0

━━━ Verification ━━━

[✓]     Installation verified: cuervo 0.2.0 (a1b2c3d4, aarch64-apple-darwin)

━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
   Update complete! 🎉
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## 🗑️ Desinstalación

### Linux / macOS

```bash
rm ~/.local/bin/cuervo
rm -rf ~/.cuervo  # Opcional: elimina configuración y datos
```

### Salida

```
Removed: /Users/username/.local/bin/cuervo
Removed: /Users/username/.cuervo/
```

### Windows (PowerShell)

```powershell
Remove-Item "$env:USERPROFILE\.local\bin\cuervo.exe"
Remove-Item -Recurse "$env:USERPROFILE\.cuervo"  # Opcional
```

---

## ⚠️ Escenarios de Error Comunes

### Error: "Command not found: cuervo"

**Salida:**

```bash
$ cuervo --version
bash: cuervo: command not found
```

**Solución:**

```bash
# Verificar instalación
ls ~/.local/bin/cuervo

# Añadir al PATH
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc

# Verificar de nuevo
cuervo --version
```

---

### Error: Checksum verification failed

**Salida del instalador:**

```
━━━ Verifying integrity ━━━

[ERROR] Checksum verification failed!
Expected: abc123def456...
Got:      xyz789uvw012...
```

**Causas:**
- Corrupción durante la descarga
- Release incompleto en GitHub
- Conexión de red interrumpida

**Solución:**

```bash
# Re-intentar instalación
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh

# Si persiste, instalar desde cargo
cargo install --git https://github.com/cuervo-ai/cuervo-cli --features tui
```

---

### Error: No binary available

**Salida del instalador:**

```
━━━ Downloading binary ━━━

[WARN]  Failed to download precompiled binary for armv7-unknown-linux-gnueabihf
[INFO]  Attempting installation via cargo-binstall...
[INFO]  cargo-binstall not found
[WARN]  No precompiled binary available for your platform.
[INFO]  Falling back to cargo install (this will compile from source, may take 2-5 minutes)...
```

**Proceso de fallback:**

```
━━━ Compiling from source ━━━

[INFO]  This may take several minutes...
    Updating git repository `https://github.com/cuervo-ai/cuervo-cli`
    Compiling cuervo-core v0.1.0
    Compiling cuervo-tools v0.1.0
    ...
    Compiling cuervo-cli v0.1.0
    Finished release [optimized] target(s) in 3m 42s
  Installing /Users/username/.cargo/bin/cuervo

[✓]     Installed via cargo install
```

---

## 💡 Tips de Instalación

### 1. Instalación silenciosa (sin interacción)

```bash
# Linux/macOS
export CUERVO_INSTALL_DIR="$HOME/.local/bin"
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh

# Windows
$env:CUERVO_INSTALL_DIR = "$env:USERPROFILE\.local\bin"
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
```

### 2. Instalación en CI/CD

```yaml
# GitHub Actions
- name: Install Cuervo CLI
  run: |
    curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
    echo "$HOME/.local/bin" >> $GITHUB_PATH

- name: Verify installation
  run: cuervo --version
```

### 3. Instalación en Dockerfile

```dockerfile
FROM ubuntu:22.04

# Instalar dependencias
RUN apt-get update && apt-get install -y curl ca-certificates

# Instalar Cuervo CLI
RUN curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh

# Añadir al PATH
ENV PATH="/root/.local/bin:${PATH}"

# Verificar
RUN cuervo --version
```

---

## 📊 Plataformas Soportadas

| Plataforma | Target | Método | Tiempo |
|------------|--------|--------|--------|
| **Ubuntu 20.04+** | x86_64-unknown-linux-gnu | Binario | ~10s |
| **Debian 11+** | x86_64-unknown-linux-gnu | Binario | ~10s |
| **Fedora 38+** | x86_64-unknown-linux-gnu | Binario | ~10s |
| **Alpine Linux** | x86_64-unknown-linux-musl | Binario | ~10s |
| **Raspberry Pi 4** | aarch64-unknown-linux-gnu | Binario | ~10s |
| **macOS 12+ Intel** | x86_64-apple-darwin | Binario | ~10s |
| **macOS 14+ M1/M2/M3/M4** | aarch64-apple-darwin | Binario | ~10s |
| **Windows 10+** | x86_64-pc-windows-msvc | Binario | ~15s |

---

## 🆘 Soporte

Si experimentas problemas durante la instalación:

1. **Revisa esta guía** para ver ejemplos de salidas esperadas
2. **Consulta [INSTALL.md](../INSTALL.md)** para troubleshooting detallado
3. **Ejecuta diagnósticos**: `cuervo doctor`
4. **Abre un issue**: [GitHub Issues](https://github.com/cuervo-ai/cuervo-cli/issues)
5. **Pregunta en Discussions**: [GitHub Discussions](https://github.com/cuervo-ai/cuervo-cli/discussions)

---

**[◀ Volver a Instalación](../INSTALL.md)** | **[Guía de Inicio Rápido ▶](../QUICKSTART.md)**
