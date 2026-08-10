self.addEventListener('push', (event) => {
  event.waitUntil(self.registration.showNotification('Open Cowork', {
    body: 'A run needs your attention.',
    icon: '/favicon.png',
    badge: '/favicon.png',
    tag: 'open-cowork-run-event',
    data: { path: '/' },
  }))
})

self.addEventListener('notificationclick', (event) => {
  event.notification.close()
  event.waitUntil((async () => {
    const clients = await self.clients.matchAll({ type: 'window', includeUncontrolled: true })
    const existing = clients[0]
    if (existing) {
      await existing.focus()
      return
    }
    await self.clients.openWindow('/')
  })())
})
