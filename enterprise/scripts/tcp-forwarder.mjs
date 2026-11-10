import { createConnection, createServer } from 'node:net'

const listenHost = process.env.COWORK_FORWARD_LISTEN_HOST ?? '127.0.0.1'
const listenPort = Number(process.env.COWORK_FORWARD_LISTEN_PORT ?? 19101)
const targetHost = process.env.COWORK_FORWARD_TARGET_HOST ?? '127.0.0.1'
const targetPort = Number(process.env.COWORK_FORWARD_TARGET_PORT ?? 19000)

if (![listenPort, targetPort].every((port) => Number.isInteger(port) && port > 0 && port < 65_536)) {
  throw new Error('forwarder ports must be valid TCP port numbers')
}

const sockets = new Set()
const server = createServer((client) => {
  const upstream = createConnection({ host: targetHost, port: targetPort })
  sockets.add(client)
  sockets.add(upstream)
  const cleanup = () => {
    sockets.delete(client)
    sockets.delete(upstream)
    client.destroy()
    upstream.destroy()
  }
  client.on('error', cleanup)
  upstream.on('error', cleanup)
  client.on('close', cleanup)
  upstream.on('close', cleanup)
  client.pipe(upstream)
  upstream.pipe(client)
})

function shutdown() {
  for (const socket of sockets) socket.destroy()
  server.close(() => process.exit(0))
  setTimeout(() => process.exit(1), 2_000).unref()
}

process.on('SIGINT', shutdown)
process.on('SIGTERM', shutdown)
server.listen(listenPort, listenHost)
