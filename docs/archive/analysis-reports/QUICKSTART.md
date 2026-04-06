# 🚀 Guía de Inicio Rápido - Cuervo CLI

**Comienza a usar Cuervo CLI en menos de 5 minutos.**

---

## Paso 1️⃣: Instalación (10 segundos)

Elige tu sistema operativo:

### 🐧 Linux / 🍎 macOS

Abre una terminal y ejecuta:

```bash
curl -fsSL https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.sh | sh
```

<details>
<summary>💡 ¿Qué hace este comando?</summary>

- Detecta tu sistema operativo y arquitectura
- Descarga el binario precompilado correcto
- Lo instala en `~/.local/bin/cuervo`
- Configura tu PATH automáticamente
- Verifica que todo funcione

</details>

### 🪟 Windows

Abre **PowerShell** y ejecuta:

```powershell
iwr -useb https://raw.githubusercontent.com/cuervo-ai/cuervo-cli/main/scripts/install-binary.ps1 | iex
```

<details>
<summary>💡 ¿Qué hace este comando?</summary>

- Detecta tu arquitectura (x64/x86)
- Descarga el binario precompilado
- Lo instala en `%USERPROFILE%\.local\bin\cuervo.exe`
- Agrega la ubicación a tu PATH
- Verifica que todo funcione

</details>

### ✅ Verificar Instalación

Después de instalar, verifica:

```bash
cuervo --version
```

**Salida esperada:**
```
cuervo 0.1.0 (f8f41dd0, aarch64-apple-darwin)
```

Si el comando no se encuentra, recarga tu terminal:

```bash
# Bash
source ~/.bashrc

# Zsh (macOS por defecto)
source ~/.zshrc

# PowerShell (Windows)
# Cierra y abre una nueva ventana
```

---

## Paso 2️⃣: Configuración Inicial (1 minuto)

### Opción A: Configuración Asistida (Recomendada)

```bash
cuervo init
```

El asistente te guiará paso a paso para:
- ✅ Elegir tu proveedor de IA preferido
- ✅ Configurar tus API keys
- ✅ Establecer preferencias por defecto
- ✅ Validar que todo funcione

### Opción B: Configuración Manual

Si prefieres configurar manualmente:

```bash
# Configurar Anthropic Claude
cuervo auth login anthropic
# Te pedirá tu API key: sk-ant-...

# Configurar OpenAI
cuervo auth login openai
# Te pedirá tu API key: sk-...

# Configurar DeepSeek
cuervo auth login deepseek
# Te pedirá tu API key: sk-...

# Configurar Ollama (modelos locales)
cuervo auth login ollama
# No requiere API key
```

### ¿Dónde obtener API Keys?

| Proveedor | Crear API Key | Documentación |
|-----------|---------------|---------------|
| **Anthropic (Claude)** | [console.anthropic.com](https://console.anthropic.com/) | [Docs](https://docs.anthropic.com/) |
| **OpenAI (GPT)** | [platform.openai.com/api-keys](https://platform.openai.com/api-keys) | [Docs](https://platform.openai.com/docs) |
| **DeepSeek** | [platform.deepseek.com](https://platform.deepseek.com/) | [Docs](https://platform.deepseek.com/docs) |
| **Ollama (Local)** | [ollama.com/download](https://ollama.com/download) | No requiere API key |

---

## Paso 3️⃣: Primer Uso (30 segundos)

### 🎯 Modo Chat Interactivo

La forma más fácil de empezar:

```bash
cuervo
```

Esto abre un **REPL (Read-Eval-Print Loop)** donde puedes chatear con la IA:

```
┌─────────────────────────────────────────────────┐
│ Cuervo CLI v0.1.0                               │
│ Provider: anthropic | Model: claude-sonnet-4.5  │
└─────────────────────────────────────────────────┘

You> Explica qué es un closure en Rust

Assistant> Un closure en Rust es una función anónima que puede capturar
variables de su entorno. Se definen con la sintaxis |parametros| { cuerpo }...

You> Muéstrame un ejemplo

Assistant> Por supuesto, aquí un ejemplo práctico:

```rust
fn main() {
    let x = 5;
    let suma = |y| x + y;  // Closure que captura x
    println!("{}", suma(3));  // Imprime: 8
}
```

You> /exit
```

**Comandos útiles del REPL:**
- `/help` - Muestra ayuda
- `/model` - Cambia el modelo de IA
- `/clear` - Limpia la pantalla
- `/exit` - Salir

### 💬 Modo Chat Directo

Para preguntas rápidas sin REPL:

```bash
cuervo chat "¿Cuál es la diferencia entre Vec y &[T] en Rust?"
```

### 🖥️ Modo TUI (Interfaz Completa)

Para una experiencia visual completa:

```bash
cuervo --tui
```

Características del TUI:
- **Zona de Prompt**: Editor multilínea (Enter = nueva línea, Ctrl+Enter = enviar)
- **Zona de Actividad**: Scroll de conversación y respuestas
- **Zona de Estado**: Tokens, costo, modelo actual
- **Panel Lateral**: Métricas, contexto, razonamiento (F2 para toggle)
- **Overlays**: Paleta de comandos (Ctrl+P), búsqueda (Ctrl+F), ayuda (F1)

**Atajos de teclado principales:**
- `Ctrl+Enter`: Enviar mensaje
- `Ctrl+K`: Limpiar prompt
- `Ctrl+C` / `Ctrl+D`: Salir
- `F1`: Ayuda
- `F2`: Toggle panel lateral
- `F3`: Cambiar modo UI (Minimal → Standard → Expert)
- `Ctrl+P`: Paleta de comandos
- `Ctrl+F`: Buscar en conversación

---

## Paso 4️⃣: Casos de Uso Comunes

### 📝 Generación de Código

```bash
cuervo chat "Genera una función en Rust que lea un archivo CSV y lo convierta en JSON"
```

### 🐛 Debug y Explicación

```bash
cuervo chat "Explica este error: cannot borrow \`*x\` as mutable more than once"
```

### 📚 Documentación

```bash
cuervo chat "Documenta esta función con ejemplos" < mi_funcion.rs
```

### 🔄 Refactoring

```bash
cuervo chat "Refactoriza este código para usar async/await" < legacy_code.rs
```

### 🧪 Generación de Tests

```bash
cuervo chat "Genera tests unitarios para esta función" < my_func.rs
```

---

## 🎓 Comandos Esenciales

### Información del Sistema

```bash
# Ver configuración actual
cuervo config show

# Ver estado de proveedores
cuervo status

# Ejecutar diagnósticos
cuervo doctor

# Ver herramientas disponibles
cuervo tools list
```

### Gestión de Sesiones

```bash
# Ver historial de sesiones
cuervo memory list

# Buscar en memoria semántica
cuervo memory search "patrón builder rust"

# Exportar sesión actual
cuervo trace export sesion-01.json
```

### Cambiar Proveedor/Modelo

```bash
# Cambiar modelo por defecto
cuervo config set default_model gpt-4o

# Cambiar proveedor por defecto
cuervo config set default_provider openai

# Usar modelo específico para una consulta
cuervo chat --model claude-opus-4-6 "Tu pregunta aquí"
```

---

## 🆘 Solución de Problemas Comunes

### ❌ "Command not found: cuervo"

**Problema:** El binario no está en tu PATH.

**Solución:**

```bash
# Verificar que el binario existe
ls ~/.local/bin/cuervo

# Añadir a PATH manualmente
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

### ❌ "API key not configured"

**Problema:** No has configurado tu API key.

**Solución:**

```bash
# Configurar interactivamente
cuervo auth login <proveedor>

# O usando variable de entorno
export ANTHROPIC_API_KEY="sk-ant-..."
```

### ❌ "Connection refused" (Ollama)

**Problema:** Ollama no está corriendo.

**Solución:**

```bash
# Asegúrate de que Ollama esté instalado y corriendo
ollama serve

# Verificar que funcione
curl http://localhost:11434/api/tags
```

### ❌ Instalación falla en Windows

**Problema:** SmartScreen bloquea el instalador.

**Solución:**

1. Click en "More info"
2. Click en "Run anyway"
3. El binario es seguro (puedes verificar el checksum SHA256)

### 📖 Más Ayuda

- **Documentación completa:** [INSTALL.md](INSTALL.md)
- **Guía de usuario:** [docs/USER_GUIDE.md](docs/USER_GUIDE.md)
- **Issues:** [github.com/cuervo-ai/cuervo-cli/issues](https://github.com/cuervo-ai/cuervo-cli/issues)

---

## 🎯 Próximos Pasos

Ahora que tienes Cuervo CLI instalado y configurado:

1. **Explora el TUI mode:** `cuervo --tui`
2. **Lee la documentación:** [docs/](docs/)
3. **Únete a la comunidad:** [Discussions](https://github.com/cuervo-ai/cuervo-cli/discussions)
4. **Contribuye:** [CONTRIBUTING.md](CONTRIBUTING.md)

---

## 💡 Tips Profesionales

### Alias útiles

Añade a tu `.bashrc` o `.zshrc`:

```bash
# Alias rápidos
alias c='cuervo'
alias ct='cuervo --tui'
alias cc='cuervo chat'

# Funciones útiles
ask() {
  cuervo chat "$*"
}

explain() {
  cat "$1" | cuervo chat "Explica este código"
}

refactor() {
  cat "$1" | cuervo chat "Refactoriza este código para $2"
}
```

Uso:
```bash
ask "¿Cómo funciona async en Rust?"
explain src/main.rs
refactor old_code.rs "usar async/await"
```

### Configuración avanzada

Edita `~/.cuervo/config.toml`:

```toml
[general]
default_provider = "deepseek"
default_model = "deepseek-chat"
max_tokens = 8192
temperature = 0.0

[display]
ui_mode = "expert"  # minimal, standard, expert
theme = "default"

[tools]
confirm_destructive = true  # Pedir confirmación en operaciones peligrosas
timeout_secs = 120
dry_run = false  # true = modo simulación, no ejecuta comandos

[security]
pii_detection = true  # Detectar información personal sensible
audit_enabled = true  # Guardar logs de auditoría
```

---

**¡Listo! 🎉 Ya estás usando Cuervo CLI.**

¿Preguntas? Abre un [issue](https://github.com/cuervo-ai/cuervo-cli/issues) o inicia una [discusión](https://github.com/cuervo-ai/cuervo-cli/discussions).
