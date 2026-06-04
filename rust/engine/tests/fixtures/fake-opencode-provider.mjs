import { createServer } from 'node:http'

const args = process.argv.slice(2)

if (args[0] === 'serve') {
  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url ?? '/', 'http://127.0.0.1')
      const body = await readBody(request)

      if (request.method === 'POST' && url.pathname === '/session') {
        sendJSON(response, { id: 'opencode-session-structured' })
        return
      }

      if (request.method === 'POST' && url.pathname === '/session/opencode-session-structured/message') {
        const prompt = body?.parts?.[0]?.text ?? ''

        if (!body?.format) {
          sendJSON(response, {
            message: {
              role: 'assistant',
              parts: [{ type: 'text', text: `fake opencode: ${prompt}` }],
            },
            request: body,
            usage: { input_tokens: 12, output_tokens: 7, total_tokens: 19 },
          })
          return
        }

        const structured = { summary: 'structured opencode summary', prompt }

        if (prompt.includes('tool-state-structured')) {
          sendJSON(response, {
            message: {
              parts: [
                { type: 'tool', tool: 'StructuredOutput', state: { input: { summary: 'structured opencode summary' } } },
              ],
            },
            usage: { input_tokens: 6, output_tokens: 4, total_tokens: 10 },
          })
          return
        }

        sendJSON(response, {
          structured,
          request: body,
          usage: { input_tokens: 12, output_tokens: 7, total_tokens: 19 },
        })
        return
      }

      response.writeHead(404, { 'content-type': 'application/json' })
      response.end(JSON.stringify({ error: `not found: ${request.method} ${url.pathname}` }))
    } catch (error) {
      response.writeHead(500, { 'content-type': 'application/json' })
      response.end(JSON.stringify({ error: String(error?.message ?? error) }))
    }
  })

  server.listen(0, '127.0.0.1', () => {
    const address = server.address()
    console.log(`opencode server listening on http://127.0.0.1:${address.port}`)
  })

  process.on('SIGTERM', () => server.close(() => process.exit(0)))
  process.on('SIGINT', () => server.close(() => process.exit(0)))
  setTimeout(() => {}, 2 ** 31 - 1)
} else {
  const prompt = args[args.length - 1]

  if (prompt.includes('fail')) {
    console.error('nope')
    process.exit(7)
  }

  if (prompt.includes('usage-nested')) {
    // Return usage inside a nested structure — must not be double-counted.
    console.log(JSON.stringify({ type: 'session', sessionID: 'opencode-session-1' }))
    console.log(JSON.stringify({
      type: 'usage',
      data: {
        usage: { input_tokens: 5, output_tokens: 3, cache_read_input_tokens: 4 }
      }
    }))
    console.log(JSON.stringify({ type: 'message', message: { role: 'assistant', content: [{ type: 'text', text: 'nested usage result' }] } }))
    process.exit(0)
  }

  if (prompt.includes('event-properties')) {
    // Current SDK output can nest assistant payloads under event.properties.
    console.log(JSON.stringify({
      type: 'event',
      event: {
        properties: {
          sessionID: 'opencode-session-2',
          message: {
            role: 'assistant',
            content: [{ type: 'text', text: 'event properties result' }],
          },
          usage: {
            input_tokens: 5,
            output_tokens: 3,
            cache_read_input_tokens: 4,
          },
        },
      },
    }))
    process.exit(0)
  }

  console.log(JSON.stringify({ type: 'session', sessionID: 'opencode-session-1' }))

  if (prompt.includes('cache-alias')) {
    // Return usage with cache sub-object (opencode normalizeUsage reads cache.read / cache.write).
    console.log(JSON.stringify({
      type: 'message', message: { role: 'assistant', content: [{ type: 'text', text: 'cache alias result' }] }
    }))
    console.log(JSON.stringify({
      type: 'usage',
      usage: {
        input_tokens: 10,
        output_tokens: 4,
        cache: { read: 2, write: 3 },
      }
    }))
    process.exit(0)
  }

  if (prompt.includes('tool-use-alongside-text')) {
    // Emit a message with both a tool_use block and a text block.
    // The provider must return the text string, NOT the tool_use object.
    console.log(JSON.stringify({
      type: 'message',
      message: {
        role: 'assistant',
        content: [
          { type: 'tool_use', id: 'tu_1', name: 'read_file', input: { path: '/tmp/x' } },
          { type: 'text', text: 'tool use result text' },
        ],
      },
    }))
    console.log(JSON.stringify({ type: 'usage', usage: { input_tokens: 3, output_tokens: 2, total_tokens: 5 } }))
    process.exit(0)
  }

  const output = `fake opencode: ${prompt}`

  console.log(JSON.stringify({ type: 'message', message: { role: 'assistant', content: [{ type: 'text', text: output }] } }))
  console.log(JSON.stringify({ type: 'usage', usage: { input_tokens: 12, output_tokens: 7, total_tokens: 19 } }))
}

function readBody(request) {
  return new Promise((resolve, reject) => {
    let text = ''
    request.setEncoding('utf8')
    request.on('data', chunk => { text += chunk })
    request.on('error', reject)
    request.on('end', () => {
      try {
        resolve(text ? JSON.parse(text) : undefined)
      } catch (error) {
        reject(error)
      }
    })
  })
}

function sendJSON(response, body) {
  response.writeHead(200, { 'content-type': 'application/json' })
  response.end(JSON.stringify(body))
}
