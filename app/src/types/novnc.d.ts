declare module '@novnc/novnc' {
  export type RFBCredentials = {
    username?: string
    password?: string
    target?: string
  }

  export type RFBOptions = {
    credentials?: RFBCredentials
    shared?: boolean
    repeaterID?: string
    wsProtocols?: string[]
  }

  export type RFBEventMap = {
    connect: CustomEvent<Record<string, never>>
    disconnect: CustomEvent<{ clean: boolean }>
    clipboard: CustomEvent<{ text: string }>
    credentialsrequired: CustomEvent<{ types: string[] }>
    securityfailure: CustomEvent<{ status: number; reason?: string }>
  }

  export default class RFB extends EventTarget {
    constructor(target: HTMLElement, urlOrChannel: string | WebSocket | RTCDataChannel, options?: RFBOptions)

    viewOnly: boolean
    clipViewport: boolean
    scaleViewport: boolean
    resizeSession: boolean
    showDotCursor: boolean
    background: string
    qualityLevel: number
    compressionLevel: number
    readonly capabilities: Record<string, boolean>

    disconnect(): void
    sendCredentials(credentials: RFBCredentials): void
    sendCtrlAltDel(): void
    clipboardPasteFrom(text: string): void
    focus(options?: FocusOptions): void

    addEventListener<K extends keyof RFBEventMap>(
      type: K,
      listener: (this: RFB, event: RFBEventMap[K]) => void,
      options?: boolean | AddEventListenerOptions,
    ): void
    removeEventListener<K extends keyof RFBEventMap>(
      type: K,
      listener: (this: RFB, event: RFBEventMap[K]) => void,
      options?: boolean | EventListenerOptions,
    ): void
  }
}
