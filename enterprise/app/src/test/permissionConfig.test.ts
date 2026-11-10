import { describe, it, expect } from 'vitest'
import type { PermissionConfig } from '../stores/chatStore'

describe('PermissionConfig', () => {
  it('should create a valid PermissionConfig', () => {
    const config: PermissionConfig = {
      mode: 'bypass',
      allowedDirectories: ['/home/user/project', '/tmp'],
    }

    expect(config.mode).toBe('bypass')
    expect(config.allowedDirectories).toHaveLength(2)
    expect(config.allowedDirectories[0]).toBe('/home/user/project')
  })

  it('should allow empty allowedDirectories', () => {
    const config: PermissionConfig = {
      mode: 'default',
      allowedDirectories: [],
    }

    expect(config.allowedDirectories).toHaveLength(0)
  })

  it('should support all permission modes', () => {
    const modes: Array<'default' | 'plan' | 'bypass' | 'strict'> = [
      'default',
      'plan',
      'bypass',
      'strict',
    ]

    modes.forEach((mode) => {
      const config: PermissionConfig = {
        mode,
        allowedDirectories: [],
      }
      expect(config.mode).toBe(mode)
    })
  })
})