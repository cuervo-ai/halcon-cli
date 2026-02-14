# 🚀 Métodos de Instalación de Cuervo CLI

Guía de referencia rápida de todos los métodos de instalación disponibles.

---

## 📦 Método 1: Binarios Precompilados (Recomendado)

**⏱️ Tiempo: ~10 segundos**

### Linux / macOS

```bash
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
```

**Lo que hace:**
- ✅ Detecta automáticamente OS, arquitectura y libc
- ✅ Descarga el binario correcto desde GitHub Releases
- ✅ Verifica checksum SHA256
- ✅ Instala en `~/.local/bin/cuervo`
- ✅ Configura PATH automáticamente

### Windows (PowerShell)

```powershell
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
```

**Lo que hace:**
- ✅ Detecta arquitectura (x64/x86)
- ✅ Descarga el ZIP correcto
- ✅ Verifica checksum
- ✅ Instala en `%USERPROFILE%\.local\bin\cuervo.exe`
- ✅ Configura PATH en variables de entorno de usuario

### Personalizar directorio de instalación

```bash
# Unix
export CUERVO_INSTALL_DIR="$HOME/bin"
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh

# Windows
$env:CUERVO_INSTALL_DIR = "C:\Tools"
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
```

---

## 📦 Método 2: cargo install

**⏱️ Tiempo: ~2-5 minutos**

### Requisitos Previos

```bash
# Instalar Rust (si no lo tienes)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Instalación

```bash
# Desde el repositorio Git (recomendado)
cargo install --git https://github.com/cuervo-ai/cuervo-cli --features tui --locked

# Desde crates.io (cuando esté publicado)
cargo install cuervo-cli --features tui
```

**Ventajas:**
- Siempre obtiene la última versión
- Compilado específicamente para tu sistema
- No requiere binario precompilado

**Desventajas:**
- Requiere Rust instalado
- Toma varios minutos compilar
- Requiere espacio para dependencias

---

## 📦 Método 3: cargo-binstall

**⏱️ Tiempo: ~15 segundos**

### Requisitos Previos

```bash
# Instalar cargo-binstall
cargo install cargo-binstall
```

### Instalación

```bash
cargo binstall cuervo-cli
```

**Ventajas:**
- Más rápido que `cargo install`
- Descarga binarios precompilados
- Integrado con cargo

---

## 📦 Método 4: Descarga Manual

**⏱️ Tiempo: ~2 minutos**

### Pasos

1. **Descarga** desde [GitHub Releases](https://github.com/cuervo-ai/cuervo-cli/releases/latest)

2. **Selecciona tu plataforma:**
   - `cuervo-x86_64-unknown-linux-gnu.tar.gz` (Linux x64 glibc)
   - `cuervo-x86_64-unknown-linux-musl.tar.gz` (Linux x64 musl/Alpine)
   - `cuervo-aarch64-unknown-linux-gnu.tar.gz` (Linux ARM64)
   - `cuervo-x86_64-apple-darwin.tar.gz` (macOS Intel)
   - `cuervo-aarch64-apple-darwin.tar.gz` (macOS M1/M2/M3/M4)
   - `cuervo-x86_64-pc-windows-msvc.zip` (Windows x64)

3. **Descarga el checksum** (archivo `.sha256`)

4. **Verifica:**
   ```bash
   # Linux/macOS
   sha256sum -c cuervo-*.tar.gz.sha256

   # Windows (PowerShell)
   (Get-FileHash cuervo-*.zip).Hash -eq (Get-Content cuervo-*.zip.sha256).Split()[0]
   ```

5. **Extrae:**
   ```bash
   # Linux/macOS
   tar xzf cuervo-*.tar.gz

   # Windows
   Expand-Archive cuervo-*.zip
   ```

6. **Instala:**
   ```bash
   # Linux/macOS
   mv cuervo ~/.local/bin/
   chmod +x ~/.local/bin/cuervo

   # Windows
   move cuervo.exe %USERPROFILE%\.local\bin\
   ```

---

## 📦 Método 5: Desde Código Fuente

**⏱️ Tiempo: ~5-10 minutos**

### Para Desarrollo

```bash
# Clonar repositorio
git clone https://github.com/cuervo-ai/cuervo-cli.git
cd cuervo-cli

# Compilar (debug - rápido)
cargo build --features tui

# Compilar (release - optimizado)
cargo build --release --features tui

# Ejecutar sin instalar
cargo run --features tui -- --help

# Instalar desde código local
cargo install --path crates/cuervo-cli --features tui
```

---

## 📦 Instalación en Entornos Especiales

### Docker

```dockerfile
FROM ubuntu:22.04

RUN apt-get update && apt-get install -y curl ca-certificates && \
    curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh

ENV PATH="/root/.local/bin:${PATH}"

RUN cuervo --version
```

### GitHub Actions

```yaml
steps:
  - name: Install Cuervo CLI
    run: |
      curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
      echo "$HOME/.local/bin" >> $GITHUB_PATH

  - name: Verify
    run: cuervo --version
```

### GitLab CI

```yaml
install_cuervo:
  script:
    - curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
    - export PATH="$HOME/.local/bin:$PATH"
    - cuervo --version
```

---

## ✅ Verificación Post-Instalación

```bash
# Verificar versión
cuervo --version

# Ejecutar diagnósticos
cuervo doctor

# Mostrar ayuda
cuervo --help
```

---

## 🔄 Actualización

### Binarios Precompilados

Re-ejecuta el instalador:

```bash
# Linux/macOS
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh

# Windows
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
```

### cargo install

```bash
cargo install --git https://github.com/cuervo-ai/cuervo-cli --features tui --force
```

---

## 🗑️ Desinstalación

### Binarios

```bash
# Linux/macOS
rm ~/.local/bin/cuervo
rm -rf ~/.cuervo  # Opcional: elimina config y datos

# Windows
Remove-Item "$env:USERPROFILE\.local\bin\cuervo.exe"
Remove-Item -Recurse "$env:USERPROFILE\.cuervo"  # Opcional
```

### cargo install

```bash
cargo uninstall cuervo-cli
rm -rf ~/.cuervo  # Opcional
```

---

## 🆘 Troubleshooting

### "Command not found: cuervo"

```bash
# Verificar que existe
ls ~/.local/bin/cuervo

# Añadir a PATH
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

### "Permission denied"

```bash
chmod +x ~/.local/bin/cuervo
```

### Checksum verification fails

```bash
# Re-descargar
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh

# O instalar desde cargo
cargo install --git https://github.com/cuervo-ai/cuervo-cli --features tui
```

---

## 📊 Comparación de Métodos

| Método | Tiempo | Requisitos | Ventaja Principal |
|--------|--------|------------|-------------------|
| **Binario precompilado** | ~10s | curl/wget | ✅ Más rápido |
| **cargo-binstall** | ~15s | Rust + cargo-binstall | Integrado con cargo |
| **cargo install** | ~2-5min | Rust | Siempre actualizado |
| **Manual** | ~2min | Ninguno | Control total |
| **Desde código** | ~5-10min | Rust + Git | Desarrollo |

---

## 📚 Documentación Adicional

- **[Guía de Inicio Rápido](../QUICKSTART.md)** - Tutorial paso a paso
- **[Guía de Instalación Completa](../INSTALL.md)** - Detalles y troubleshooting
- **[Ejemplos de Instalación](../docs/INSTALLATION_EXAMPLES.md)** - Salidas esperadas
- **[Guía de Releases](../RELEASE.md)** - Para mantenedores

---

## 🌍 Plataformas Soportadas

| Plataforma | Arquitectura | Estado |
|-----------|--------------|--------|
| Linux (Ubuntu, Debian, Fedora) | x86_64 (glibc) | ✅ Tier 1 |
| Linux (Alpine) | x86_64 (musl) | ✅ Tier 1 |
| Linux | ARM64 / aarch64 | ✅ Tier 1 |
| macOS | Intel (x86_64) | ✅ Tier 1 |
| macOS | Apple Silicon (M1/M2/M3/M4) | ✅ Tier 1 |
| Windows | x64 | ✅ Tier 1 |

---

**Última actualización:** 2026-02-14

**¿Problemas?** Abre un [issue](https://github.com/cuervo-ai/cuervo-cli/issues) o consulta la [documentación completa](../INSTALL.md).
