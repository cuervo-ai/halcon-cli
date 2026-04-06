# Backend SSE Streaming Implementation Guide

## Overview

This guide shows how to implement **reliable SSE streaming** for LLM chat endpoints that work through Cloudflare, Azure, and other proxies.

**Target**: Backend developers implementing `/v1/llm/chat` or similar streaming endpoints.

---

## 🎯 Requirements

A production-ready SSE backend must:

1. ✅ **Send proper headers** (disable buffering)
2. ✅ **Flush after each event** (force immediate send)
3. ✅ **Send keep-alive comments** (prevent proxy timeout)
4. ✅ **Handle backpressure** (slow clients)
5. ✅ **Clean up on disconnect** (cancel LLM generation)

---

## 1. Proper Headers

### Node.js / Express

```javascript
app.post('/v1/llm/chat', async (req, res) => {
  // ── Critical SSE Headers ────────────────────────────────────────────
  res.setHeader('Content-Type', 'text/event-stream');
  res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
  res.setHeader('Connection', 'keep-alive');
  res.setHeader('X-Accel-Buffering', 'no'); // Nginx/Azure: disable buffering

  // CORS (if needed)
  res.setHeader('Access-Control-Allow-Origin', '*');

  // HTTP 200 + headers sent immediately
  res.writeHead(200);

  // ... streaming logic below
});
```

### Python / FastAPI

```python
from fastapi import FastAPI
from fastapi.responses import StreamingResponse

app = FastAPI()

@app.post("/v1/llm/chat")
async def chat(request: ChatRequest):
    async def event_generator():
        # ... yield events here
        pass

    return StreamingResponse(
        event_generator(),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache, no-store, must-revalidate",
            "X-Accel-Buffering": "no",
            "Connection": "keep-alive",
        }
    )
```

### Rust / Axum

```rust
use axum::{
    response::{Response, sse::{Event, Sse}},
    http::StatusCode,
};
use futures::stream::{self, Stream};

async fn chat_handler(
    Json(req): Json<ChatRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let stream = stream::iter(events).map(|e| Ok(Event::default().data(e)));

    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}
```

**Why these headers?**
- `text/event-stream`: SSE content type
- `no-cache`: Prevents CDN/proxy caching
- `X-Accel-Buffering: no`: Disables Nginx/Azure buffering
- `Connection: keep-alive`: Long-lived connection

---

## 2. Event Format

SSE events follow this format:

```
data: {"type":"token","content":"Hello"}\n\n
data: {"type":"token","content":" world"}\n\n
data: [DONE]\n\n
```

**Rules**:
- Each event starts with `data: `
- Event ends with `\n\n` (two newlines)
- Comments start with `: ` (used for keep-alive)

### Node.js Implementation

```javascript
function sendEvent(res, eventData) {
  const json = JSON.stringify(eventData);
  res.write(`data: ${json}\n\n`);
  res.flush(); // ← Critical! Forces immediate send
}

// Usage:
sendEvent(res, { type: 'token', content: 'Hello' });
sendEvent(res, { type: 'token', content: ' world' });
sendEvent(res, '[DONE]');
```

### Python Implementation

```python
async def event_generator():
    for chunk in stream:
        event_data = {"type": "token", "content": chunk}
        yield f"data: {json.dumps(event_data)}\n\n"

    yield "data: [DONE]\n\n"
```

---

## 3. Keep-Alive (Critical!)

**Problem**: Cloudflare/proxies timeout idle connections after 30-100s.

**Solution**: Send SSE comments every 15-30s.

### Node.js

```javascript
app.post('/v1/llm/chat', async (req, res) => {
  // ... headers setup

  // ── Keep-Alive Timer ────────────────────────────────────────────────
  const keepAliveInterval = setInterval(() => {
    if (res.writableEnded) {
      clearInterval(keepAliveInterval);
      return;
    }
    res.write(': keep-alive\n\n'); // SSE comment (ignored by client)
    res.flush();
  }, 15000); // Every 15s

  // ── Main Streaming Loop ─────────────────────────────────────────────
  try {
    for await (const chunk of llmStream) {
      sendEvent(res, { type: 'token', content: chunk });
    }
    sendEvent(res, '[DONE]');
  } finally {
    clearInterval(keepAliveInterval);
    res.end();
  }
});
```

### Python / FastAPI

```python
async def event_generator():
    last_event_time = time.time()

    async for chunk in llm_stream:
        yield f"data: {json.dumps({'type':'token','content':chunk})}\n\n"
        last_event_time = time.time()

        # Send keep-alive if no events for 15s
        if time.time() - last_event_time > 15:
            yield ": keep-alive\n\n"
            last_event_time = time.time()

    yield "data: [DONE]\n\n"
```

### Rust / Axum

```rust
Sse::new(stream)
    .keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive")
    )
```

**Why 15s?**
- Cloudflare default timeout: 30-100s
- 15s gives 2x safety margin
- Keeps connection alive without spam

---

## 4. Flush After Each Event

**Critical**: Without `flush()`, events are buffered in:
1. Node.js internal buffer (16KB default)
2. Nginx/Azure proxy buffer (4-8KB default)
3. HTTP/2 DATA frame buffer (variable)

### Node.js

```javascript
res.write(`data: ${json}\n\n`);
res.flush(); // ← Bypass internal buffer
```

**If you see errors**: `res.flush is not a function`

```javascript
// Enable compression middleware properly:
const compression = require('compression');
app.use(compression({ flush: require('zlib').constants.Z_SYNC_FLUSH }));
```

### Python / Flask

```python
@app.route('/chat', methods=['POST'])
def chat():
    def generate():
        for chunk in stream:
            yield f"data: {chunk}\n\n"

    return Response(
        stream_with_context(generate()),
        mimetype='text/event-stream'
    )
```

`stream_with_context()` auto-flushes.

### Python / Starlette

```python
async def event_generator():
    for chunk in stream:
        yield f"data: {chunk}\n\n"
        await asyncio.sleep(0)  # Yield control to flush
```

---

## 5. Handle Client Disconnect

**Problem**: Client disconnects, but backend keeps generating (wastes API tokens).

**Solution**: Detect disconnect and cancel LLM generation.

### Node.js

```javascript
app.post('/v1/llm/chat', async (req, res) => {
  const abortController = new AbortController();

  // Detect client disconnect
  req.on('close', () => {
    console.log('Client disconnected, cancelling LLM');
    abortController.abort();
  });

  try {
    // Pass abort signal to LLM provider
    const stream = await openai.chat.completions.create({
      model: 'gpt-4o-mini',
      messages: req.body.messages,
      stream: true,
    }, {
      signal: abortController.signal, // ← Cancel on disconnect
    });

    for await (const chunk of stream) {
      if (abortController.signal.aborted) break;
      sendEvent(res, chunk);
    }
  } catch (err) {
    if (err.name === 'AbortError') {
      console.log('LLM generation cancelled');
    } else {
      throw err;
    }
  } finally {
    clearInterval(keepAliveInterval);
    res.end();
  }
});
```

### Python / FastAPI

```python
from fastapi import Request

@app.post("/chat")
async def chat(request: Request, body: ChatRequest):
    async def event_generator():
        try:
            async for chunk in llm_stream:
                # Check if client disconnected
                if await request.is_disconnected():
                    print("Client disconnected, cancelling")
                    break

                yield f"data: {chunk}\n\n"
        except asyncio.CancelledError:
            print("LLM generation cancelled")

    return StreamingResponse(event_generator(), media_type="text/event-stream")
```

---

## 6. Backpressure Handling

**Problem**: LLM generates faster than slow client can consume.

**Solution**: Monitor buffer size, pause if full.

### Node.js

```javascript
async function sendEventWithBackpressure(res, data) {
  const canContinue = res.write(`data: ${JSON.stringify(data)}\n\n`);
  res.flush();

  if (!canContinue) {
    // Buffer full — wait for drain
    await new Promise(resolve => res.once('drain', resolve));
  }
}
```

### Python

```python
async def event_generator():
    buffer_size = 0

    async for chunk in llm_stream:
        event = f"data: {chunk}\n\n"
        buffer_size += len(event)

        yield event

        # Pause if buffer > 64KB
        if buffer_size > 65536:
            await asyncio.sleep(0.1)
            buffer_size = 0
```

---

## 7. Error Handling

Send errors as SSE events (don't just disconnect).

```javascript
try {
  // ... streaming
} catch (error) {
  sendEvent(res, {
    type: 'error',
    code: 'llm_error',
    message: error.message,
  });
} finally {
  res.end();
}
```

---

## 8. Complete Example (Node.js + OpenAI)

```javascript
const express = require('express');
const OpenAI = require('openai');

const app = express();
const openai = new OpenAI({ apiKey: process.env.OPENAI_API_KEY });

app.use(express.json());

app.post('/v1/llm/chat', async (req, res) => {
  // ── Headers ─────────────────────────────────────────────────────────
  res.setHeader('Content-Type', 'text/event-stream');
  res.setHeader('Cache-Control', 'no-cache, no-store, must-revalidate');
  res.setHeader('Connection', 'keep-alive');
  res.setHeader('X-Accel-Buffering', 'no');
  res.writeHead(200);

  // ── Keep-Alive ──────────────────────────────────────────────────────
  const keepAlive = setInterval(() => {
    if (!res.writableEnded) {
      res.write(': keep-alive\n\n');
      res.flush();
    }
  }, 15000);

  // ── Disconnect Handling ─────────────────────────────────────────────
  const abortController = new AbortController();
  req.on('close', () => {
    console.log('Client disconnected');
    abortController.abort();
    clearInterval(keepAlive);
  });

  // ── LLM Streaming ───────────────────────────────────────────────────
  try {
    const stream = await openai.chat.completions.create({
      model: req.body.model || 'gpt-4o-mini',
      messages: req.body.messages,
      stream: true,
    }, {
      signal: abortController.signal,
    });

    for await (const chunk of stream) {
      if (abortController.signal.aborted) break;

      const delta = chunk.choices[0]?.delta?.content;
      if (delta) {
        res.write(`data: ${JSON.stringify({ type: 'token', content: delta })}\n\n`);
        res.flush();
      }
    }

    res.write('data: [DONE]\n\n');
    res.flush();

  } catch (error) {
    if (error.name !== 'AbortError') {
      res.write(`data: ${JSON.stringify({ type: 'error', message: error.message })}\n\n`);
      res.flush();
    }
  } finally {
    clearInterval(keepAlive);
    res.end();
  }
});

app.listen(3000, () => console.log('Server running on :3000'));
```

---

## Testing

### 1. Manual Test with curl

```bash
curl -N -H "Content-Type: application/json" \
     -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"count to 5 slowly"}],"stream":true}' \
     http://localhost:3000/v1/llm/chat
```

**Expected**:
```
data: {"type":"token","content":"1"}

data: {"type":"token","content":"2"}

: keep-alive

data: {"type":"token","content":"3"}
```

**Failure**: All events arrive at once after 5+ seconds

### 2. Test Disconnect Handling

```bash
# Start request in background
curl -N http://localhost:3000/v1/llm/chat ... &
PID=$!

# Kill after 2s
sleep 2 && kill $PID
```

**Expected**: Server logs `Client disconnected, cancelling LLM`

### 3. Load Test

```bash
npm install -g autocannon

autocannon -c 10 -d 30 -m POST \
  -H "Content-Type: application/json" \
  -b '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}],"stream":true}' \
  http://localhost:3000/v1/llm/chat
```

**Check**: No memory leaks, all connections close cleanly

---

## Common Issues

### Issue: Events arrive in batches

**Cause**: Missing `res.flush()` or `X-Accel-Buffering: no`
**Fix**: Add both

### Issue: Connection drops after 30s

**Cause**: No keep-alive comments
**Fix**: Send `: keep-alive\n\n` every 15s

### Issue: Memory leak on disconnect

**Cause**: LLM stream not cancelled
**Fix**: Use AbortController / AbortSignal

### Issue: Works locally, fails in production

**Cause**: Nginx/Azure buffering
**Fix**: Add `X-Accel-Buffering: no` header

---

## Deployment Checklist

- [ ] Headers: `text/event-stream`, `no-cache`, `X-Accel-Buffering: no`
- [ ] Flush after each event (`res.flush()`)
- [ ] Keep-alive comments every 15s
- [ ] Disconnect handling (AbortController)
- [ ] Error events (don't just disconnect)
- [ ] Tested with curl `-N` flag
- [ ] Tested disconnect (client abort mid-stream)
- [ ] Load tested (10+ concurrent streams)

---

## Additional Resources

- **SSE Spec**: https://html.spec.whatwg.org/multipage/server-sent-events.html
- **Nginx Buffering**: https://nginx.org/en/docs/http/ngx_http_proxy_module.html#proxy_buffering
- **Cloudflare Streaming**: https://developers.cloudflare.com/workers/examples/streaming/

---

## Summary

**Minimum viable SSE backend** (5 lines):
```javascript
res.setHeader('Content-Type', 'text/event-stream');
res.setHeader('X-Accel-Buffering', 'no');
res.setHeader('Cache-Control', 'no-cache');
res.write(`data: ${json}\n\n`);
res.flush();
```

**Production-ready** (add):
- Keep-alive every 15s
- Disconnect detection
- Error handling
- Backpressure
