import assert from 'node:assert/strict'
import test from 'node:test'

import { resolveNpmInvocation } from './process-utils.mjs'

test('uses the npm CLI from the active package-manager process when available', () => {
  assert.deepEqual(resolveNpmInvocation({
    platform: 'win32',
    npmExecPath: 'C:\\npm\\npm-cli.js',
    nodeExecutable: 'C:\\node\\node.exe',
  }), {
    command: 'C:\\node\\node.exe',
    args: ['C:\\npm\\npm-cli.js'],
  })
})

test('falls back to the Windows command processor when no npm CLI path is present', () => {
  assert.deepEqual(resolveNpmInvocation({
    platform: 'win32',
    npmExecPath: '',
    commandShell: 'C:\\Windows\\System32\\cmd.exe',
  }), {
    command: 'C:\\Windows\\System32\\cmd.exe',
    args: ['/d', '/s', '/c', 'npm'],
  })
})

test('uses npm directly on non-Windows platforms', () => {
  assert.deepEqual(resolveNpmInvocation({ platform: 'linux', npmExecPath: '' }), {
    command: 'npm',
    args: [],
  })
})
