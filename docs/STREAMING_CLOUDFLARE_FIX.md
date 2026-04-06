# Fixing SSE Streaming Through Cloudflare

## Problem

Cloudflare buffers HTTP responses by default, which breaks Server-Sent Events (SSE) streaming:

- **Symptom**: "network connection lost" errors
- **Cause**: Cloudflare accumulates SSE events and delivers them in batches
- **Impact**: No real-time feedback, poor UX

## Root Cause

```
Client → Cloudflare → Backend
         ↑ buffers here
```

Cloudflare's default behavior:
1. **Buffers responses > 1MB** before forwarding
2. **No streaming** for responses without `X-Accel-Buffering: no`
3. **Timeouts** on idle connections (30-100s without keep-alive)

---

## Solution 1: Cloudflare Page Rules (Recommended)

### Step 1: Create a Page Rule

Go to **Cloudflare Dashboard** → **Rules** → **Page Rules** → **Create Page Rule**

**Pattern**: `*api.cenzontle.app/v1/llm/chat*`

**Settings**:
```
Cache Level: Bypass
Disable Apps: On
Disable Performance: On
```

**Why**: Forces Cloudflare to pass-through without buffering.

### Step 2: Enable HTTP/1.1 for SSE Endpoints

Go to **Network** → **HTTP/2 to Origin**

- **Disable** for SSE endpoints (or use separate subdomain)
- HTTP/2 multiplexing can cause buffering issues

---

## Solution 2: Cloudflare Workers (Advanced)

Create a worker to explicitly disable buffering:

```javascript
export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);

    // Only intercept SSE endpoints
    if (!url.pathname.includes('/v1/llm/chat')) {
      return fetch(request);
    }

    // Forward request to origin
    const response = await fetch(request, {
      cf: {
        cacheTtl: 0,
        cacheEverything: false,
      }
    });

    // Return streaming response with anti-buffering headers
    return new Response(response.body, {
      status: response.status,
      headers: {
        ...Object.fromEntries(response.headers),
        'Cache-Control': 'no-cache, no-store, must-revalidate',
        'X-Accel-Buffering': 'no',
        'Connection': 'keep-alive',
      },
    });
  },
};
```

**Deploy**:
```bash
npx wrangler deploy
```

---

## Solution 3: Disable Cloudflare (Temporary Debug)

To confirm Cloudflare is the issue:

1. **Bypass Cloudflare**: Use origin IP directly
   ```
   curl -v https://ORIGIN_IP/v1/llm/chat -H "Host: api.cenzontle.app"
   ```

2. **Check TTFB** (Time To First Byte):
   - **Without Cloudflare**: < 500ms
   - **With buffering**: > 5s (batched)

3. **If streaming works without Cloudflare** → confirmed proxy buffering

---

## Solution 4: Use Cloudflare Stream (Enterprise)

Cloudflare Enterprise has native SSE support:

```
Cloudflare Dashboard → Speed → Stream
Enable: "Server-Sent Events Optimization"
```

**Cost**: $200/month minimum

---

## Verification

### 1. Test SSE Streaming

```bash
curl -N -H "Authorization: Bearer $TOKEN" \
     -H "Accept: text/event-stream" \
     -H "Cache-Control: no-cache" \
     https://api.cenzontle.app/v1/llm/chat \
     -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"count to 10"}],"stream":true}'
```

**Expected**: Events arrive **incrementally** (one per second)
**Failure**: All events arrive at once after 10+ seconds

### 2. Check Response Headers

```bash
curl -I https://api.cenzontle.app/v1/llm/chat
```

**Look for**:
```
CF-Cache-Status: DYNAMIC
X-Accel-Buffering: no
Content-Type: text/event-stream
```

### 3. Monitor TTFB

Run Halcon CLI with tracing:

```bash
RUST_LOG=debug halcon -p cenzontle chat "test streaming"
```

**Look for**:
```
SSE first chunk received ttfb_ms=450    ← ✅ Good (< 1s)
⚠️  SSE first chunk delayed >5s         ← ❌ Buffering detected
```

---

## Backend Requirements

Your backend must also:

1. **Send proper headers**:
   ```
   Content-Type: text/event-stream
   Cache-Control: no-cache
   X-Accel-Buffering: no
   Connection: keep-alive
   ```

2. **Flush after each event**:
   ```javascript
   res.write(`data: ${JSON.stringify(chunk)}\n\n`);
   res.flush(); // Critical!
   ```

3. **Send keep-alive comments** (every 15-30s):
   ```javascript
   setInterval(() => {
     res.write(': keep-alive\n\n');
     res.flush();
   }, 15000);
   ```

See `STREAMING_BACKEND_GUIDE.md` for full implementation.

---

## Common Issues

### Issue: Still seeing delays after Cloudflare fix

**Cause**: Azure App Gateway also buffers
**Fix**: Set `X-Accel-Buffering: no` on origin response

### Issue: Connection drops after 30s

**Cause**: No SSE keep-alive
**Fix**: Backend must send `: keep-alive\n\n` every 15-30s

### Issue: Works in dev, fails in production

**Cause**: Production uses HTTP/2, dev uses HTTP/1.1
**Fix**: Cloudflare → Network → Disable HTTP/2 for SSE endpoints

---

## Summary

**Quick Fix** (5 min):
1. Add Cloudflare Page Rule: `Cache Level: Bypass`
2. Verify with `curl -N` test

**Permanent Fix** (30 min):
1. Cloudflare Page Rule + Worker
2. Backend: Add `X-Accel-Buffering: no` header
3. Backend: Send `: keep-alive\n\n` every 15s

**Cost**: $0 (Page Rules are free on all plans)
