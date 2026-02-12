# Cuervo CLI - Secure Docker Container
# Multi-stage build for minimal, secure container

# Stage 1: Builder
FROM rust:1.75-slim AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for build
RUN useradd -m -u 1000 builder
USER builder
WORKDIR /home/builder

# Copy source code
COPY --chown=builder:builder . .

# Build in release mode with security optimizations
RUN cargo build --release \
    --target-dir /home/builder/target \
    --features "security,audit" \
    && strip /home/builder/target/release/cuervo

# Stage 2: Runtime
FROM gcr.io/distroless/cc-debian12:nonroot

# Security labels
LABEL org.opencontainers.image.title="Cuervo CLI" \
      org.opencontainers.image.description="AI-powered CLI for software development" \
      org.opencontainers.image.version="0.1.0" \
      org.opencontainers.image.created="2026-02-07T00:00:00Z" \
      org.opencontainers.image.source="https://github.com/cuervo-ai/cuervo-cli" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.authors="Cuervo AI Team <security@cuervo.ai>" \
      org.opencontainers.image.vendor="Cuervo AI" \
      org.opencontainers.image.base.name="gcr.io/distroless/cc-debian12:nonroot"

# Security scanning metadata
LABEL security.scanning.enabled="true" \
      security.compliance.frameworks="gdpr,soc2,iso27001" \
      security.certifications.target="SOC2,ISO27001"

# Copy binary from builder
COPY --from=builder /home/builder/target/release/cuervo /usr/local/bin/cuervo

# Copy default configuration
COPY --from=builder /home/builder/config/default.toml /etc/cuervo/default.toml

# Create necessary directories with proper permissions
RUN mkdir -p /home/nonroot/.cuervo \
    && chown -R nonroot:nonroot /home/nonroot/.cuervo \
    && chmod 700 /home/nonroot/.cuervo

# Set working directory
WORKDIR /workspace

# Switch to non-root user
USER nonroot

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD ["cuervo", "doctor", "--quiet"] || exit 1

# Security hardening
# - Run as non-root user
# - Use distroless base image
# - No shell access
# - Read-only filesystem (except necessary directories)
# - Resource limits applied at runtime

# Runtime security configuration
ENV RUST_BACKTRACE=0 \
    RUST_LOG=warn \
    CUERVO_SECURITY_LEVEL=strict \
    CUERVO_AUDIT_ENABLED=true \
    CUERVO_PII_DETECTION=true

# Volume for persistent data (optional)
VOLUME ["/home/nonroot/.cuervo"]

# Entrypoint
ENTRYPOINT ["cuervo"]

# Default command (interactive chat)
CMD ["chat"]

# Build arguments for security scanning
ARG BUILD_DATE
ARG VCS_REF
ARG VERSION

# Security metadata for SBOM generation
LABEL org.opencontainers.image.revision=$VCS_REF \
      org.opencontainers.image.version=$VERSION \
      org.opencontainers.image.build-date=$BUILD_DATE

# Security best practices implemented:
# 1. Multi-stage build to reduce image size
# 2. Non-root user execution
# 3. Distroless base image (no shell, minimal packages)
# 4. Binary stripping to reduce attack surface
# 5. Health checks for container monitoring
# 6. Resource limits (applied at runtime)
# 7. Read-only filesystem where possible
# 8. Security labels for scanning tools
# 9. Environment variables for security configuration
# 10. Volume for persistent data isolation

# To build securely:
# docker build \
#   --build-arg VERSION=$(git describe --tags) \
#   --build-arg VCS_REF=$(git rev-parse HEAD) \
#   --build-arg BUILD_DATE=$(date -u +'%Y-%m-%dT%H:%M:%SZ') \
#   --tag cuervo-cli:latest \
#   --file Dockerfile .

# To scan for vulnerabilities:
# docker scan cuervo-cli:latest

# To run with security constraints:
# docker run --rm \
#   --read-only \
#   --tmpfs /tmp \
#   --memory 512m \
#   --cpus 1 \
#   --security-opt no-new-privileges \
#   --security-opt seccomp=security/seccomp.json \
#   -v cuervo-data:/home/nonroot/.cuervo \
#   -v $(pwd):/workspace:ro \
#   cuervo-cli:latest