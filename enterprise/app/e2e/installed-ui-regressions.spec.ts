import AxeBuilder from '@axe-core/playwright'
import { expect, test } from '@playwright/test'

test.beforeEach(async ({ context }) => {
  await context.addInitScript(() => {
    localStorage.clear()
    localStorage.setItem('open-cowork.language', new URLSearchParams(location.search).get('language') ?? 'en')
  })
})

for (const width of [360, 420, 899, 900]) {
  test(`navigation modal boundary and keyboard at ${width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 800 })
    await page.goto('/settings')
    await expect(page.locator('.settings-view')).toBeVisible()
    const trigger = page.getByRole('button', { name: 'Open main menu' })
    await trigger.click()
    const menu = page.getByRole('dialog', { name: 'Main menu' })
    await expect(menu).toBeVisible()
    expect((await new AxeBuilder({ page }).include('#app-menu-drawer').withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze()).violations).toEqual([])
    await page.screenshot({ path: testInfo.outputPath(`menu-${width}.png`) })
    if (width < 900) {
      await expect(menu).toHaveAttribute('aria-modal', 'true')
      await expect(page.getByTestId('app-menu-backdrop')).toBeVisible()
      const close = menu.getByRole('button', { name: 'Close menu' })
      await close.focus()
      await page.keyboard.press('Shift+Tab')
      await expect(menu.getByRole('button', { name: /Dark theme|Light theme/ })).toBeFocused()
      await page.keyboard.press('Tab')
      await expect(close).toBeFocused()
      await page.keyboard.press('Escape')
      await expect(menu).toHaveCount(0)
      await expect(trigger).toBeFocused()
      await trigger.click()
    } else {
      await expect(menu).not.toHaveAttribute('aria-modal', 'true')
      await expect(page.getByTestId('app-menu-backdrop')).toHaveCount(0)
    }
    await menu.getByRole('button', { name: /^AI Sandbox/ }).click()
    await expect(page).toHaveURL(/section=sandbox/)
    if (width < 900) {
      await expect(menu).toHaveCount(0)
      await expect(page.locator('.shell-title')).toBeFocused()
      await trigger.click()
    } else await expect(menu).toBeVisible()
    await menu.getByRole('searchbox').fill('Browser')
    await menu.getByRole('button', { name: /Browser/ }).first().click()
    await expect(page).toHaveURL(/\/browser$/)
    if (width < 900) {
      await expect(menu).toHaveCount(0)
      await expect(page.locator('.shell-title')).toBeFocused()
      await trigger.click()
    } else await expect(menu).toBeVisible()
    await menu.getByRole('button', { name: /^Tasks/ }).click()
    await expect(page).toHaveURL(/\/tasks$/)
    if (width < 900) await expect(menu).toHaveCount(0)
    else await expect(menu).toBeVisible()
  })
}

for (const width of [360, 420, 899]) {
  for (const language of ['en', 'de']) {
    test(`browser and task layout in ${language} at ${width}px`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 800 })
      await page.goto(`/browser?language=${language}`)
      await expect(page.locator('.developer-browser-view')).toBeVisible()
      const url = page.getByRole('textbox', { name: 'Browser URL', exact: true }).or(page.getByRole('textbox', { name: 'Browser-URL', exact: true }))
      await expect(url).toHaveValue('https://example.com')
      const geometry = await page.locator('.developer-browser-toolbar').evaluate((toolbar) => {
        const form = toolbar.querySelector('form')!.getBoundingClientRect()
        const mode = toolbar.querySelector('.developer-browser-mode-toggle')!.getBoundingClientRect()
        const input = toolbar.querySelector('input')!.getBoundingClientRect()
        return { formTop: form.top, modeBottom: mode.bottom, inputWidth: input.width, right: form.right, viewport: innerWidth }
      })
      expect(geometry.inputWidth).toBeGreaterThan(120)
      expect(geometry.right).toBeLessThanOrEqual(geometry.viewport)
      if (width <= 620) expect(geometry.formTop).toBeGreaterThanOrEqual(geometry.modeBottom)
      const violations = (await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze()).violations
      expect(violations).toEqual([])
      await testInfo.attach(`browser-${language}-${width}`, { body: await page.screenshot({ path: testInfo.outputPath(`browser-${language}-${width}.png`) }), contentType: 'image/png' })

      await page.goto(`/tasks?language=${language}`)
      await expect(page.locator('.task-create-heading')).toBeVisible()
      await page.locator('.task-create-panel-toggle').click()
      await expect(page.locator('.task-create-panel')).toHaveClass(/is-collapsed/)
      const subtitle = await page.locator('.task-create-heading p').evaluate((element) => ({
        overflow: element.scrollWidth > element.clientWidth + 1, whiteSpace: getComputedStyle(element).whiteSpace,
      }))
      expect(subtitle.overflow).toBe(false)
      expect(subtitle.whiteSpace).toBe('normal')
      await testInfo.attach(`tasks-${language}-${width}`, { body: await page.screenshot({ path: testInfo.outputPath(`tasks-${language}-${width}.png`) }), contentType: 'image/png' })
    })
  }
}

for (const language of ['en', 'de']) {
  test(`embedded server owns no second h1 in ${language}`, async ({ page }) => {
    await page.goto(`/server?language=${language}`)
    await expect(page.locator('.remote-login-card')).toBeVisible()
    await expect(page.getByRole('heading', { level: 1 })).toHaveCount(1)
    await expect(page.getByRole('heading', { level: 2, name: language === 'de' ? 'Mit Open Cowork Server verbinden' : 'Connect to Open Cowork Server' })).toBeVisible()
  })
}
