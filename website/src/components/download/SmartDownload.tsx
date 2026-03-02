import { useState, useEffect } from 'react';

interface Artifact {
  name: string;
  sha256: string;
  size: number;
  target: string;
}

interface Manifest {
  version: string;
  published_at: string;
  artifacts: Artifact[];
  checksums_url: string;
  sbom_url?: string;
}

interface SmartDownloadProps {
  releasesUrl: string;
}

type OS = 'linux' | 'macos' | 'windows' | 'unknown';
type Arch = 'x86_64' | 'aarch64' | 'unknown';

function detectPlatform(): { os: OS; arch: Arch; target: string } {
  const ua = navigator.userAgent.toLowerCase();
  const platform = navigator.platform?.toLowerCase() ?? '';

  let os: OS = 'unknown';
  let arch: Arch = 'unknown';

  if (platform.includes('win') || ua.includes('windows')) {
    os = 'windows';
  } else if (platform.includes('mac') || ua.includes('macintosh')) {
    os = 'macos';
  } else if (platform.includes('linux') || ua.includes('linux')) {
    os = 'linux';
  }

  if ('userAgentData' in navigator) {
    const uaData = (navigator as any).userAgentData;
    const fullArch = uaData?.architecture?.toLowerCase() ?? '';
    if (fullArch.includes('arm') || fullArch.includes('aarch64')) {
      arch = 'aarch64';
    } else if (fullArch.includes('x86') || fullArch.includes('amd64')) {
      arch = 'x86_64';
    }
  }

  if (arch === 'unknown') {
    arch = os === 'macos' ? 'aarch64' : 'x86_64';
  }

  const targetMap: Record<string, Record<string, string>> = {
    linux:   { x86_64: 'x86_64-unknown-linux-musl', aarch64: 'aarch64-unknown-linux-gnu', unknown: 'x86_64-unknown-linux-musl' },
    macos:   { x86_64: 'x86_64-apple-darwin',        aarch64: 'aarch64-apple-darwin',      unknown: 'aarch64-apple-darwin' },
    windows: { x86_64: 'x86_64-pc-windows-msvc',     aarch64: 'x86_64-pc-windows-msvc',    unknown: 'x86_64-pc-windows-msvc' },
    unknown: { x86_64: 'x86_64-unknown-linux-musl',  aarch64: 'x86_64-unknown-linux-musl', unknown: 'x86_64-unknown-linux-musl' },
  };

  return { os, arch, target: targetMap[os][arch] };
}

function formatBytes(bytes: number): string {
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

const OS_ICONS: Record<OS, string> = {
  macos:   '  macOS',
  linux:   '  Linux',
  windows: '  Windows',
  unknown: '  Linux',
};

export default function SmartDownload({ releasesUrl }: SmartDownloadProps) {
  const [manifest, setManifest]   = useState<Manifest | null>(null);
  const [error, setError]         = useState<string | null>(null);
  const [loading, setLoading]     = useState(true);
  const [platform, setPlatform]   = useState<ReturnType<typeof detectPlatform> | null>(null);
  const [activeTab, setActiveTab] = useState<'sha256' | 'cosign' | 'script'>('sha256');
  const [copied, setCopied]       = useState(false);

  useEffect(() => {
    setPlatform(detectPlatform());
    fetch(`${releasesUrl}/latest/manifest.json`)
      .then(r => { if (!r.ok) throw new Error(`HTTP ${r.status}`); return r.json(); })
      .then((data: Manifest) => { setManifest(data); setLoading(false); })
      .catch(err => { setError(err.message); setLoading(false); });
  }, [releasesUrl]);

  const copyToClipboard = (text: string) => {
    navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    }).catch(() => {});
  };

  // ── Loading state ────────────────────────────────────────────────────────────
  if (loading) {
    return (
      <div className="rounded-2xl overflow-hidden"
           style={{ background: 'rgba(7,4,1,0.92)', border: '1px solid rgba(200,80,0,0.18)' }}>
        <div className="px-6 py-8 flex items-center justify-center gap-3">
          <div className="w-2 h-2 rounded-full animate-pulse" style={{ background: 'var(--c-p)' }} />
          <span className="text-sm font-mono" style={{ color: 'var(--c-t3)' }}>
            Fetching latest release…
          </span>
        </div>
      </div>
    );
  }

  // ── Error state ──────────────────────────────────────────────────────────────
  if (error || !manifest || !platform) {
    return (
      <div className="rounded-2xl overflow-hidden"
           style={{ background: 'rgba(196,20,0,0.06)', border: '1px solid rgba(196,20,0,0.25)' }}>
        <div className="px-6 py-6">
          <p className="font-semibold mb-2" style={{ color: '#e04020' }}>
            Could not load release information
          </p>
          <p className="text-sm mb-3" style={{ color: 'var(--c-t3)' }}>
            {error ?? 'Unknown error'}
          </p>
          <a href="https://releases.cli.cuervo.cloud"
             className="text-sm underline underline-offset-2"
             style={{ color: 'var(--c-gold)' }}>
            Browse all releases ↗
          </a>
        </div>
      </div>
    );
  }

  const { target, os, arch } = platform;
  const ext          = os === 'windows' ? 'zip' : 'tar.gz';
  const artifactName = `halcon-${manifest.version}-${target}.${ext}`;
  const artifact     = manifest.artifacts.find(a => a.name === artifactName);
  const downloadUrl  = `${releasesUrl}/latest/${artifactName}`;
  const publishDate  = manifest.published_at
    ? new Date(manifest.published_at).toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric' })
    : '';

  const archLabel = arch === 'aarch64' ? 'ARM64' : 'x86_64';

  const hasSha256 = !!artifact?.sha256 && artifact.sha256.length === 64;
  const hasSize   = !!artifact && artifact.size > 0;

  const tabContent: Record<string, string> = {
    sha256: hasSha256
      ? `${artifact!.sha256}  ${artifactName}`
      : `# SHA-256 pending for this platform.\n# Check: ${releasesUrl}/latest/checksums.txt`,
    cosign: `cosign verify-blob \\
  --signature ${artifactName}.sig \\
  --certificate ${artifactName}.pem \\
  --certificate-identity-regexp 'https://github.com/cuervo-ai/halcon-cli' \\
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \\
  ${artifactName}`,
    script: os === 'windows'
      ? `iwr -useb https://cli.cuervo.cloud/install.ps1 | iex`
      : `curl -sSfL https://cli.cuervo.cloud/install.sh | sh`,
  };

  // ── Main render ──────────────────────────────────────────────────────────────
  return (
    <div className="rounded-2xl overflow-hidden"
         style={{ background: 'rgba(7,4,1,0.92)', border: '1px solid rgba(200,80,0,0.18)' }}>

      {/* ── Header bar ─────────────────────────────────────────────────────── */}
      <div className="px-6 py-4 flex items-center justify-between border-b"
           style={{ borderColor: 'rgba(200,80,0,0.12)', background: 'rgba(17,8,3,0.70)' }}>
        <div className="flex items-center gap-3">
          <span className="w-2 h-2 rounded-full" style={{ background: 'var(--c-emerald)' }} />
          <span className="text-xs font-mono" style={{ color: 'var(--c-t3)' }}>
            Halcón CLI · v{manifest.version}
          </span>
        </div>
        <div className="flex items-center gap-3">
          <span className="text-xs font-mono" style={{ color: 'var(--c-t4)' }}>
            {publishDate}
          </span>
          <span className="text-xs px-2 py-0.5 rounded font-mono"
                style={{ background: 'rgba(232,82,0,0.15)', color: 'var(--c-p)', border: '1px solid rgba(232,82,0,0.30)' }}>
            LATEST
          </span>
        </div>
      </div>

      {/* ── Platform detection badge ────────────────────────────────────────── */}
      <div className="px-6 py-3 border-b flex items-center gap-2"
           style={{ borderColor: 'rgba(200,80,0,0.08)' }}>
        <span className="text-xs" style={{ color: 'var(--c-t4)' }}>Detected:</span>
        <span className="text-xs px-2 py-0.5 rounded font-mono font-semibold"
              style={{ background: 'rgba(245,160,0,0.08)', color: 'var(--c-gold)', border: '1px solid rgba(245,160,0,0.20)' }}>
          {OS_ICONS[os]} · {archLabel}
        </span>
        <span className="text-xs font-mono ml-1" style={{ color: 'var(--c-t4)' }}>
          → {target}
        </span>
      </div>

      {/* ── Download button ─────────────────────────────────────────────────── */}
      <div className="px-6 py-6">
        <a href={downloadUrl}
           download
           className="group relative flex items-center justify-center gap-3 rounded-xl px-6 py-4 font-bold text-lg transition-all duration-200 w-full overflow-hidden"
           style={{
             background: 'linear-gradient(135deg, #e85200 0%, #c44100 100%)',
             boxShadow: '0 4px 24px rgba(232,82,0,0.35), inset 0 1px 0 rgba(255,255,255,0.12)',
             color: '#fff',
           }}
           onMouseOver={e => { (e.currentTarget as HTMLElement).style.boxShadow = '0 6px 32px rgba(232,82,0,0.55), inset 0 1px 0 rgba(255,255,255,0.15)'; }}
           onMouseOut={e  => { (e.currentTarget as HTMLElement).style.boxShadow = '0 4px 24px rgba(232,82,0,0.35), inset 0 1px 0 rgba(255,255,255,0.12)'; }}
        >
          <svg className="w-5 h-5 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                  d="M4 16v1a3 3 0 003 3h10a3 3 0 003-3v-1m-4-4l-4 4m0 0l-4-4m4 4V4" />
          </svg>
          <span>
            Download for {OS_ICONS[os]}
          </span>
          {hasSize && (
            <span className="text-sm font-normal opacity-80">
              {formatBytes(artifact!.size)}
            </span>
          )}
        </a>

        {/* Artifact filename */}
        <div className="mt-2.5 text-center font-mono text-xs" style={{ color: 'var(--c-t4)' }}>
          {artifactName}
        </div>

        {/* No artifact warning */}
        {!artifact && (
          <div className="mt-3 text-center text-xs px-4 py-2 rounded-lg"
               style={{ background: 'rgba(245,160,0,0.06)', border: '1px solid rgba(245,160,0,0.15)', color: 'var(--c-gold)' }}>
            ⚠ Pre-built binary for this platform coming soon.
            Contact <a href="mailto:support@cuervo.cloud" className="underline">support@cuervo.cloud</a> to request a build.
          </div>
        )}
      </div>

      {/* ── Verification tabs ───────────────────────────────────────────────── */}
      <div className="border-t" style={{ borderColor: 'rgba(200,80,0,0.10)' }}>
        {/* Tab bar */}
        <div className="flex px-6 pt-4 gap-1">
          {(['sha256', 'cosign', 'script'] as const).map(tab => (
            <button key={tab} onClick={() => setActiveTab(tab)}
              className="px-3 py-1.5 rounded-t text-xs font-medium transition-all duration-150"
              style={{
                background:   activeTab === tab ? 'rgba(232,82,0,0.12)' : 'transparent',
                color:        activeTab === tab ? 'var(--c-p)' : 'var(--c-t4)',
                border:       activeTab === tab ? '1px solid rgba(232,82,0,0.25)' : '1px solid transparent',
                borderBottom: 'none',
              }}>
              {tab === 'sha256' ? 'SHA-256' : tab === 'cosign' ? 'Cosign' : 'Install script'}
            </button>
          ))}
        </div>

        {/* Tab content */}
        <div className="mx-6 mb-6 rounded-b rounded-tr overflow-hidden"
             style={{ border: '1px solid rgba(200,80,0,0.15)', background: 'rgba(4,2,0,0.70)' }}>
          <div className="flex items-start justify-between gap-3 p-4">
            <pre className="font-mono text-xs leading-relaxed overflow-x-auto flex-1 whitespace-pre-wrap break-all"
                 style={{ color: 'var(--c-emerald)' }}>
              {tabContent[activeTab]}
            </pre>
            <button onClick={() => copyToClipboard(tabContent[activeTab])}
              title="Copy"
              className="flex-shrink-0 mt-0.5 p-1.5 rounded transition-colors"
              style={{ color: copied ? 'var(--c-emerald)' : 'var(--c-t4)' }}>
              {copied ? (
                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2.5} d="M5 13l4 4L19 7" />
                </svg>
              ) : (
                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
                        d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
                </svg>
              )}
            </button>
          </div>
        </div>
      </div>

      {/* ── Footer ─────────────────────────────────────────────────────────── */}
      <div className="px-6 py-3 border-t flex items-center justify-between"
           style={{ borderColor: 'rgba(200,80,0,0.08)', background: 'rgba(4,2,0,0.50)' }}>
        <a href={manifest.checksums_url ?? '#'}
           className="flex items-center gap-1.5 text-xs transition-colors"
           style={{ color: 'var(--c-t4)' }}
           onMouseOver={e => { (e.currentTarget as HTMLElement).style.color = 'var(--c-t2)'; }}
           onMouseOut={e  => { (e.currentTarget as HTMLElement).style.color = 'var(--c-t4)'; }}>
          <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={1.5}
                  d="M9 12l2 2 4-4m5.618-4.016A11.955 11.955 0 0112 2.944a11.955 11.955 0 01-8.618 3.04A12.02 12.02 0 003 9c0 5.591 3.824 10.29 9 11.622 5.176-1.332 9-6.03 9-11.622 0-1.042-.133-2.052-.382-3.016z" />
          </svg>
          checksums.txt (SHA-256)
        </a>
        <a href={(manifest as any).release_notes ?? 'https://cuervo.cloud/changelog'}
           className="text-xs transition-colors"
           style={{ color: 'var(--c-t4)' }}
           onMouseOver={e => { (e.currentTarget as HTMLElement).style.color = 'var(--c-gold)'; }}
           onMouseOut={e  => { (e.currentTarget as HTMLElement).style.color = 'var(--c-t4)'; }}>
          Release notes ↗
        </a>
      </div>
    </div>
  );
}
