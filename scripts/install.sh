#!/bin/bash
# Cuervo CLI - Script de Instalación

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}╔══════════════════════════════════════════════════════════╗${NC}"
echo -e "${BLUE}║             Instalación de Cuervo CLI                    ║${NC}"
echo -e "${BLUE}╚══════════════════════════════════════════════════════════╝${NC}"
echo ""

# Verificar Rust
echo -e "${YELLOW}[1/5] Verificando instalación de Rust...${NC}"
if ! command -v rustc &> /dev/null; then
    echo -e "${RED}Rust no está instalado.${NC}"
    echo "Instalando Rust via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
else
    echo -e "${GREEN}✓ Rust está instalado.${NC}"
fi

# Verificar versión de Rust
RUST_VERSION=$(rustc --version | cut -d' ' -f2)
REQUIRED_VERSION="1.75.0"
if [ "$(printf '%s\n' "$REQUIRED_VERSION" "$RUST_VERSION" | sort -V | head -n1)" = "$REQUIRED_VERSION" ]; then
    echo -e "${GREEN}✓ Versión de Rust: $RUST_VERSION (>= $REQUIRED_VERSION)${NC}"
else
    echo -e "${YELLOW}⚠ Versión de Rust: $RUST_VERSION (se requiere >= $REQUIRED_VERSION)${NC}"
    echo "Actualizando Rust..."
    rustup update stable
fi

# Clonar o actualizar repositorio
echo -e "\n${YELLOW}[2/5] Preparando código fuente...${NC}"
if [ -d "cuervo-cli" ]; then
    echo "Actualizando repositorio existente..."
    cd cuervo-cli
    git pull origin main
else
    echo "Clonando repositorio..."
    git clone https://github.com/cuervo-ai/cuervo-cli
    cd cuervo-cli
fi

# Compilar
echo -e "\n${YELLOW}[3/5] Compilando Cuervo CLI...${NC}"
echo "Esto puede tomar varios minutos..."
cargo build --release

# Instalar
echo -e "\n${YELLOW}[4/5] Instalando...${NC}"
sudo cp target/release/cuervo /usr/local/bin/
echo -e "${GREEN}✓ Cuervo CLI instalado en /usr/local/bin/cuervo${NC}"

# Configuración inicial
echo -e "\n${YELLOW}[5/5] Configuración inicial...${NC}"
if [ ! -d "$HOME/.cuervo" ]; then
    mkdir -p "$HOME/.cuervo"
    echo -e "${GREEN}✓ Directorio de configuración creado: ~/.cuervo/${NC}"
fi

if [ ! -f "$HOME/.cuervo/config.toml" ]; then
    echo "Creando configuración inicial..."
    cat > "$HOME/.cuervo/config.toml" << 'EOF'
[general]
default_provider = "anthropic"
default_model = "claude-sonnet-4-5-20250929"
max_tokens = 8192
temperature = 0.0

[models.providers.ollama]
enabled = true
api_base = "http://localhost:11434"
default_model = "llama3.2"

[tools]
confirm_destructive = true
timeout_secs = 120

[security]
pii_detection = true
audit_enabled = true
EOF
    echo -e "${GREEN}✓ Configuración inicial creada${NC}"
fi

# Verificar instalación
echo -e "\n${BLUE}══════════════════════════════════════════════════════════${NC}"
echo -e "${GREEN}¡Instalación completada!${NC}"
echo ""
echo "Para usar Cuervo CLI:"
echo "1. Configura tu API key:"
echo "   $ cuervo auth login anthropic"
echo "   o exporta ANTHROPIC_API_KEY en tu entorno"
echo ""
echo "2. Inicia una sesión:"
echo "   $ cuervo"
echo ""
echo "3. Para ayuda:"
echo "   $ cuervo --help"
echo ""
echo "Documentación: https://github.com/cuervo-ai/cuervo-cli"
echo -e "${BLUE}══════════════════════════════════════════════════════════${NC}"

# Verificar que funciona
if command -v cuervo &> /dev/null; then
    echo -e "\n${GREEN}Verificando instalación...${NC}"
    cuervo --version
else
    echo -e "\n${YELLOW}Advertencia: El comando 'cuervo' no está en PATH${NC}"
    echo "Puedes ejecutarlo directamente desde:"
    echo "  $(pwd)/target/release/cuervo"
fi