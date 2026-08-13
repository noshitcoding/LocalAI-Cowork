export function resolveNpmInvocation({
  platform = process.platform,
  npmExecPath = process.env.npm_execpath,
  nodeExecutable = process.execPath,
  commandShell = process.env.ComSpec,
} = {}) {
  if (npmExecPath) {
    return { command: nodeExecutable, args: [npmExecPath] }
  }

  if (platform === 'win32') {
    return {
      command: commandShell || 'cmd.exe',
      args: ['/d', '/s', '/c', 'npm'],
    }
  }

  return { command: 'npm', args: [] }
}
