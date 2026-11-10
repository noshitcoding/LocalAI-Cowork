import { useTranslation } from 'react-i18next'

export default function EnterpriseEnrollmentReview({ checked, onChange, recovery = false }: {
  checked: boolean; onChange: (value: boolean) => void; recovery?: boolean
}) {
  const { i18n } = useTranslation()
  const t = (en: string, de: string) => i18n.language.startsWith('de') ? de : en
  return <>
    <p>{recovery
      ? t('This permits one verified continuation of the interrupted enrollment. A saved completion is reported without repeating work. The instance remains stopped; uncertain changes require further review.', 'Dies erlaubt einen geprüften Fortsetzungsversuch der unterbrochenen Übernahme. Ein gespeicherter Abschluss wird ohne erneute Ausführung gemeldet. Die Instanz bleibt gestoppt; unklare Änderungen erfordern eine weitere Prüfung.')
      : t('A fresh encrypted backup is verified before switching this instance to central task limits. Existing data remains in place. Interrupted server tasks remain available for review and are not restarted. Running or queued work and unresolved device assignments prevent enrollment. The instance stays stopped until central limits are activated for the platform.', 'Eine frische verschlüsselte Sicherung wird geprüft, bevor diese Instanz auf zentrale Aufgabenlimits umgestellt wird. Vorhandene Daten bleiben erhalten. Unterbrochene Server-Aufgaben bleiben zur Prüfung erhalten und werden nicht neu gestartet. Laufende oder eingereihte Aufgaben und ungeklärte Gerätezuweisungen verhindern die Übernahme. Die Instanz bleibt gestoppt, bis die zentralen Limits für die Plattform aktiviert sind.')}</p>
    <label><input type="checkbox" checked={checked} onChange={event => onChange(event.target.checked)} />
      {t('I authorize this enrollment review and understand that the instance remains stopped.', 'Ich bestätige diesen Übernahmevorgang und verstehe, dass die Instanz gestoppt bleibt.')}
    </label>
  </>
}
