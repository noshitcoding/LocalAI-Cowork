import { useTranslation } from 'react-i18next'
import { supportedLanguages, type AppLanguage } from '../i18n'

export default function LanguageSwitcher() {
  const { i18n, t } = useTranslation()
  const currentLanguage = (i18n.resolvedLanguage ?? i18n.language).split('-')[0] as AppLanguage

  return (
    <label className="language-switcher" title={t('language.select')}>
      <span className="sr-only">{t('language.select')}</span>
      <select
        value={supportedLanguages.some((language) => language.code === currentLanguage) ? currentLanguage : 'en'}
        aria-label={t('language.select')}
        onChange={(event) => {
          void i18n.changeLanguage(event.target.value as AppLanguage)
        }}
      >
        {supportedLanguages.map((language) => (
          <option key={language.code} value={language.code}>{language.shortLabel}</option>
        ))}
      </select>
    </label>
  )
}
