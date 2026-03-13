/**
 * Halcon CLI — Releases Worker
 *
 * Domain: releases.cli.cuervo.cloud
 *
 * Routes:
 *   /health                         → status JSON
 *   /latest/manifest.json           → build manifest from GitHub API (KV-cached)
 *   /latest/checksums.txt           → proxy from GitHub Release asset
 *   /latest/<artifact>              → proxy binary from GitHub Release
 *   /v<version>/<artifact>          → proxy binary from GitHub Release (versioned)
 *
 * Note: cli.cuervo.cloud (install.sh / install.ps1) is served by the
 *       Cloudflare Pages project "halcon-website" — not this Worker.
 *
 * Required bindings (wrangler.toml / dashboard):
 *   RELEASES_KV  — KV namespace for manifest caching
 *   GITHUB_TOKEN — Secret for GitHub API auth (private repo support)
 *   GITHUB_REPO  — Plaintext var, e.g. "cuervo-ai/halcon-cli"
 *   CACHE_TTL    — Plaintext var, seconds (default "300")
 */

const GITHUB_API = "https://api.github.com";

// Allowed CORS origins — only our own properties
const ALLOWED_ORIGINS = new Set([
  "https://cli.cuervo.cloud",
  "https://halcon.cuervo.cloud",
  "https://cuervo.cloud",
  "https://halcon-website.pages.dev",
]);

export default {
  async fetch(request, env, ctx) {
    const url  = new URL(request.url);
    const path = url.pathname;

    // Resolve config from env bindings (with safe fallbacks)
    const GITHUB_REPO = (env.GITHUB_REPO || "cuervo-ai/halcon-cli").trim();
    const CACHE_TTL   = parseInt(env.CACHE_TTL || "300", 10);

    // Force HTTPS
    if (url.protocol === "http:") {
      return Response.redirect(`https://${url.host}${url.pathname}${url.search}`, 301);
    }

    // CORS — emit header only for whitelisted origins
    const origin = request.headers.get("Origin") || "";
    const corsHeaders = ALLOWED_ORIGINS.has(origin)
      ? {
          "Access-Control-Allow-Origin":  origin,
          "Access-Control-Allow-Methods": "GET, HEAD, OPTIONS",
          "Access-Control-Allow-Headers": "Content-Type",
          "Vary": "Origin",
        }
      : { "Vary": "Origin" };

    if (request.method === "OPTIONS") {
      return new Response(null, {
        status: ALLOWED_ORIGINS.has(origin) ? 204 : 403,
        headers: corsHeaders,
      });
    }

    const secHeaders = {
      ...corsHeaders,
      "X-Content-Type-Options": "nosniff",
      "Cache-Control": `public, max-age=${CACHE_TTL}, s-maxage=${CACHE_TTL}`,
    };

    // GitHub auth — required for private repos
    const githubAuth = env.GITHUB_TOKEN
      ? { "Authorization": `Bearer ${env.GITHUB_TOKEN}` }
      : {};

    try {
      // ── /health ─────────────────────────────────────────────────────────────
      if (path === "/" || path === "/health") {
        return Response.json({
          service:   "halcon-releases",
          status:    "ok",
          repo:      GITHUB_REPO,
          endpoints: [
            "GET /health",
            "GET /latest/manifest.json",
            "GET /latest/checksums.txt",
            "GET /latest/<artifact>",
            "GET /v<version>/<artifact>",
          ],
        }, { headers: secHeaders });
      }

      // ── /latest/manifest.json ────────────────────────────────────────────────
      if (path === "/latest/manifest.json") {
        return await serveManifest(env, ctx, secHeaders, githubAuth, GITHUB_REPO, CACHE_TTL);
      }

      // ── /latest/checksums.txt ────────────────────────────────────────────────
      if (path === "/latest/checksums.txt") {
        return await serveReleaseAsset(
          "latest", "checksums.txt", secHeaders, githubAuth, GITHUB_REPO
        );
      }

      // ── /latest/<file> ───────────────────────────────────────────────────────
      const latestMatch = path.match(/^\/latest\/(.+)$/);
      if (latestMatch) {
        return await proxyReleaseAsset(
          "latest", latestMatch[1], secHeaders, githubAuth, GITHUB_REPO, CACHE_TTL
        );
      }

      // ── /v<version>/<file> ───────────────────────────────────────────────────
      const versionMatch = path.match(/^\/v([\d.]+(?:-[\w.]+)?)\/(.+)$/);
      if (versionMatch) {
        return await proxyReleaseAsset(
          `v${versionMatch[1]}`, versionMatch[2], secHeaders, githubAuth, GITHUB_REPO, CACHE_TTL
        );
      }

      return new Response("Not found", { status: 404, headers: secHeaders });

    } catch (err) {
      console.error("Worker error:", err);
      return new Response(`Internal error: ${err.message}`, {
        status: 502,
        headers: secHeaders,
      });
    }
  }
};

// ─── Serve manifest.json (KV-cached) ────────────────────────────────────────
async function serveManifest(env, ctx, headers, githubAuth, repo, cacheTtl) {
  const cacheKey = `manifest:latest:${repo}`;

  // 1. KV cache hit
  if (env.RELEASES_KV) {
    const cached = await env.RELEASES_KV.get(cacheKey, { type: "text" });
    if (cached) {
      return new Response(cached, {
        headers: { ...headers, "Content-Type": "application/json", "X-Cache": "HIT" },
      });
    }
  }

  // 2. Fetch from GitHub API
  const release = await fetchLatestRelease(githubAuth, repo);
  if (!release) {
    return Response.json(
      { error: "No releases found", hint: `Check https://api.github.com/repos/${repo}/releases/latest` },
      { status: 404, headers: { ...headers, "Content-Type": "application/json" } }
    );
  }

  // 3. Best-effort: fetch checksums to populate sha256 fields
  let checksums = "";
  const csAsset = release.assets?.find(a => a.name === "checksums.txt");
  if (csAsset) {
    try {
      const r = await downloadGitHubAsset(csAsset, githubAuth);
      if (r?.ok) checksums = await r.text();
    } catch (_) { /* non-fatal */ }
  }

  const manifest = buildManifest(release, checksums);
  const body     = JSON.stringify(manifest, null, 2);

  // 4. Populate KV cache in background
  if (env.RELEASES_KV) {
    ctx.waitUntil(
      env.RELEASES_KV.put(cacheKey, body, { expirationTtl: cacheTtl })
    );
  }

  return new Response(body, {
    headers: { ...headers, "Content-Type": "application/json", "X-Cache": "MISS" },
  });
}

// ─── Serve a release asset as full response (checksums.txt, etc.) ────────────
async function serveReleaseAsset(tag, filename, headers, githubAuth, repo) {
  const release = tag === "latest"
    ? await fetchLatestRelease(githubAuth, repo)
    : await fetchRelease(tag, githubAuth, repo);

  if (!release) {
    return new Response("Release not found", { status: 404, headers });
  }

  const asset = release.assets?.find(a => a.name === filename);
  if (!asset) {
    return new Response(`Asset '${filename}' not found`, { status: 404, headers });
  }

  const resp = await downloadGitHubAsset(asset, githubAuth);
  if (!resp?.ok) {
    return new Response(`Failed to download '${filename}'`, { status: 502, headers });
  }

  return new Response(resp.body, {
    status: 200,
    headers: {
      ...headers,
      "Content-Type": resp.headers.get("Content-Type") || "text/plain; charset=utf-8",
    },
  });
}

// ─── Proxy release binary (tar.gz / zip) ─────────────────────────────────────
async function proxyReleaseAsset(tag, filename, headers, githubAuth, repo, cacheTtl) {
  const release = tag === "latest"
    ? await fetchLatestRelease(githubAuth, repo)
    : await fetchRelease(tag, githubAuth, repo);

  if (!release) {
    return new Response("Release not found", { status: 404, headers });
  }

  const asset = release.assets?.find(a => a.name === filename);
  if (!asset) {
    return new Response(`Asset '${filename}' not found in release ${tag}`, { status: 404, headers });
  }

  const resp = await downloadGitHubAsset(asset, githubAuth);
  if (!resp?.ok) {
    return new Response(`Failed to download '${filename}'`, { status: 502, headers });
  }

  const isArchive = filename.endsWith(".tar.gz") || filename.endsWith(".zip");
  const contentType = isArchive
    ? "application/octet-stream"
    : resp.headers.get("Content-Type") || "application/octet-stream";

  return new Response(resp.body, {
    status: 200,
    headers: {
      ...headers,
      "Content-Type": contentType,
      "Content-Length": resp.headers.get("Content-Length") || "",
      "Cache-Control": `public, max-age=${cacheTtl}, s-maxage=${cacheTtl}`,
    },
  });
}

// ─── Two-step GitHub asset download (handles private repo S3 redirects) ──────
async function downloadGitHubAsset(asset, githubAuth) {
  const apiUrl = asset.url || asset.browser_download_url;
  const resp1 = await fetch(apiUrl, {
    headers: {
      "User-Agent": "halcon-releases-worker/2.0",
      "Accept":     "application/octet-stream",
      ...githubAuth,
    },
    redirect: "manual",
  });

  // GitHub redirects private assets to S3 presigned URLs.
  // We MUST NOT forward the Authorization header to S3 or it returns 400.
  if (resp1.status === 301 || resp1.status === 302) {
    const location = resp1.headers.get("Location");
    if (location) {
      return fetch(location, { headers: { "User-Agent": "halcon-releases-worker/2.0" } });
    }
  }

  return resp1.ok ? resp1 : null;
}

// ─── GitHub API helpers ───────────────────────────────────────────────────────
async function fetchLatestRelease(githubAuth, repo) {
  const resp = await fetch(`${GITHUB_API}/repos/${repo}/releases/latest`, {
    headers: {
      "User-Agent": "halcon-releases-worker/2.0",
      "Accept":     "application/vnd.github+json",
      ...githubAuth,
    },
  });
  return resp.ok ? resp.json() : null;
}

async function fetchRelease(tag, githubAuth, repo) {
  const resp = await fetch(`${GITHUB_API}/repos/${repo}/releases/tags/${tag}`, {
    headers: {
      "User-Agent": "halcon-releases-worker/2.0",
      "Accept":     "application/vnd.github+json",
      ...githubAuth,
    },
  });
  return resp.ok ? resp.json() : null;
}

// ─── Build manifest.json from GitHub release data ────────────────────────────
function buildManifest(release, checksums) {
  const version = (release.tag_name || "").replace(/^v/, "") || "unknown";

  // Parse "sha256  filename" lines from checksums.txt
  const sha256Map = {};
  for (const line of (checksums || "").split("\n")) {
    const parts = line.trim().split(/\s+/);
    if (parts.length >= 2) sha256Map[parts[1]] = parts[0];
  }

  // Filter out metadata files — keep only downloadable binary artifacts
  const SKIP_NAMES = new Set(["checksums.txt", "manifest.json"]);
  const SKIP_EXTS  = [".sha256", ".sig", ".pem", ".json", ".asc", ".txt"];

  const artifacts = (release.assets || [])
    .filter(a => {
      if (SKIP_NAMES.has(a.name)) return false;
      if (SKIP_EXTS.some(ext => a.name.endsWith(ext))) return false;
      return true;
    })
    .map(a => {
      const target = extractTarget(a.name, version);
      return {
        name:   a.name,
        target,
        os:     inferOs(target),
        arch:   inferArch(target),
        sha256: sha256Map[a.name] || "",
        size:   a.size,
        url:    `https://releases.cli.cuervo.cloud/v${version}/${a.name}`,
      };
    });

  return {
    version,
    published_at:  release.published_at,
    artifacts,
    checksums_url: `https://releases.cli.cuervo.cloud/v${version}/checksums.txt`,
    github_url:    release.html_url,
  };
}

function extractTarget(filename, version) {
  return filename
    .replace(`halcon-${version}-`, "")
    .replace(".tar.gz", "")
    .replace(".zip", "");
}

function inferOs(target) {
  if (target.includes("apple-darwin")) return "macos";
  if (target.includes("linux"))        return "linux";
  if (target.includes("windows"))      return "windows";
  return "unknown";
}

function inferArch(target) {
  if (target.startsWith("aarch64")) return "aarch64";
  if (target.startsWith("x86_64"))  return "x86_64";
  if (target.startsWith("armv7"))   return "armv7";
  return "unknown";
}
