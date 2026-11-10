export const loginFieldHelp = {
  en: {
    server: 'The address of this server. In the web app it is selected automatically; in the desktop app, use the address supplied by your administrator.',
    email: 'The email address of your account on this server. For an invitation, use the address the invitation was sent to.',
    organisation: 'Only needed for your organisation’s single sign-on (SSO). Your administrator supplies this ID. Leave it empty when signing in with email and password.',
    displayName: 'The name other members will see. You can use your real name or a recognisable display name.',
    password: 'Enter your account password. When creating an account, choose a new password with at least 12 characters.',
    confirmPassword: 'Enter the same new password again to check for typing errors.',
    secondFactor: 'If two-factor authentication is enabled, enter the current code from your authenticator app or an unused recovery code. Otherwise, leave this empty.',
    bootstrap: 'The one-time setup secret supplied by the server operator. It is only used to create the very first administrator and cannot be used for normal sign-in.',
    invitation: 'The private invitation token supplied by an administrator. It lets you create the account you were invited to use.',
  },
  de: {
    server: 'Die Adresse dieses Servers. In der Web-App ist sie automatisch ausgewählt. In der Desktop-App verwendest du die Adresse, die dir deine Administration gegeben hat.',
    email: 'Die E-Mail-Adresse deines Kontos auf diesem Server. Bei einer Einladung verwendest du die Adresse, an die die Einladung gesendet wurde.',
    organisation: 'Nur für die zentrale Anmeldung deiner Organisation (SSO) nötig. Die ID erhältst du von deiner Administration. Bei der Anmeldung mit E-Mail und Passwort bleibt das Feld leer.',
    displayName: 'Der Name, den andere Mitglieder sehen. Du kannst deinen echten Namen oder einen erkennbaren Anzeigenamen verwenden.',
    password: 'Gib dein Kontopasswort ein. Wenn du ein Konto erstellst, wähle ein neues Passwort mit mindestens 12 Zeichen.',
    confirmPassword: 'Gib das neue Passwort noch einmal ein, um Tippfehler auszuschließen.',
    secondFactor: 'Wenn die Zwei-Faktor-Anmeldung aktiviert ist, gib den aktuellen Code deiner Authenticator-App oder einen unbenutzten Wiederherstellungscode ein. Sonst bleibt das Feld leer.',
    bootstrap: 'Der einmalige Einrichtungsschlüssel vom Serverbetreiber. Er wird nur für den allerersten Administrator benötigt und ist kein Passwort für die normale Anmeldung.',
    invitation: 'Der private Einladungsschlüssel von deiner Administration. Damit erstellst du das Konto, zu dem du eingeladen wurdest.',
  },
} as const
