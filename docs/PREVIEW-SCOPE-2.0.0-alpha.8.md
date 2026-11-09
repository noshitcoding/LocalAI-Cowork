# Previewumfang · 2.0.0-alpha.8

Stand: 9. November 2026. **Releasevorbereitung; noch nicht veröffentlicht.**
Diese Abgrenzung ist der Produktvertrag der nächsten Desktop-Preview und
erweitert keine historischen Paket-, Installations- oder Produktionsnachweise.

## Erprobungsumfang

| Bereich | Im Quellstand implementiert | Grenze vor Veröffentlichung |
| --- | --- | --- |
| Workspace | Projekte, Chat, Modelle, Dateikontext und bestätigte Ausführungspläne | Modell, Rechte, Sidecar-/Serververbindung und ausgewähltes Ziel müssen eingerichtet und im tatsächlichen Paket geprüft sein. |
| Werkzeuge | MCP, Terminal, Dateiimport/-download und bewusste Werkzeugfreigaben | Externe Programme und Dienste haben eigene Abhängigkeiten und Datenrouten; keine pauschale OS-Isolationsgarantie. |
| Workflows | Typisierte Eingaben, Schrittergebnisse, Bedingungen, Zweige, relative Wartezeiten, eingefrorene Unterabläufe/Listenverarbeitung und Prüfformulare | Vollständige Rechnungs-/OCR-/Buchungskette, absolute Termine und Ereigniswarten sind nicht abgeschlossen. |
| Dokumente | Quellenansicht, geführte Dateiauswahl, optional gewählte Dokumentengine, OCR DE/EN und überprüfbare Extraktionsvorschläge | Programme bewusst einrichten; Ergebnisse prüfen. Windows-Pfadvorprüfung ist keine vollständige Langpfadunterstützung. |
| Wissen | Themenordner, Unterordner, Notizen, Importe und Graphen aus Tags/Quellen | Automatische semantische Entitätsextraktion ist offen. |

Die Aufnahme in diese Tabelle behauptet keine erfolgreiche aktuelle Installation.
Der vollständige Quellstand und das tatsächlich ausgelieferte Paket benötigen
jeweils eigene bestandene Prüfungen. Die [Release Notes](RELEASE-NOTES-2.0.0-alpha.8.md)
führen vor Veröffentlichung die konkreten Belege und verbleibenden Ausnahmen.

## Experimentell und getrennt abzunehmen

- Managed Runtime: Bootstrap-/Profil-/Dienstanbindung, registrierte Runner,
  Ergebnisabruf, Ergebnisübernahme und installierter Appweg bleiben entsprechend
  ihrer jeweiligen Produktdokumentation begrenzt. Quellenfertige Einzelschnitte
  sind kein kompletter verwalteter Betriebsablauf.
- Enterprise, Android, Mehrgerätesync, SSO/IdP, Container/Kubernetes, KMS,
  entfernte Geräte und Office-Automation behalten ihre eigene reale Einrichtung,
  Rechteprüfung und Plattform-/Betriebsabnahme. Keine Paritäts- oder
  Produktionszusage aus der Desktop-Preview ableiten.
- Vollständige OCR-Rechnungskette, Ablage/Meldung, dauerhafte Übernahme und
  automatische Seitennachladung sind keine fertigen Previewversprechen.
- Plattformen außerhalb Windows x64 und Linux x86_64 sind nicht angeboten.
  RPM ist in der V2-Tauri-Konfiguration kein Paketziel.

## Unterstützte Paketziele und Updategrenze

Windows: NSIS, aktueller Benutzer, eigene Kennung
`dev.localai.cowork.v2.preview`. Linux: DEB und AppImage mit glibc **ab 2.39**,
GTK 3 und WebKitGTK 4.1. AppImage umgeht die libc-Baseline nicht.

Signierung und Updatefeed werden ausschließlich anhand der tatsächlich
veröffentlichten Paketquittung und Metadaten beschrieben. Leere Updatekonfiguration
ist kein aktiver Updatekanal. Historische Signierwerkzeuge oder ältere Installer
sind kein aktueller Signier-/Installationsbeleg. Automatische V1-auf-V2-Updates
sind nicht Teil dieses Previewvertrags.

## V1 und Wiederherstellung

V1 v0.3.1 bleibt im [bestehenden öffentlichen Release](https://github.com/noshitcoding/LocalAI-Cowork/releases/tag/v0.3.1)
erreichbar. V1 sichern und behalten; V2 mit eigenem Profil und Kopien von
Arbeitsdateien erproben. Keine V1-Datenbank direkt mit V2 öffnen und keine
V2-Datenbank mit V1 zurückstufen.

Historienimport verwendet den gesonderten Quarantäne-/Prüfweg. Übernommene
Inhalte brauchen aktuelle Rechte und ausdrückliche Bestätigung. Alte Schlüssel,
Actor-/ACL-Daten, Runs, Freigaben, Schedules und Outbox-Operationen verleihen
keine aktuelle Ausführungsautorität. Die reale Import-/Daten-/Versionsmatrix
ist ein eigenes Gate; [Migration](MIGRATION-V1.md) und [Quarantäne](QUARANTINE.md)
gelten jeweils nur in ihrem dokumentierten und belegten Umfang.

## Verbindlicher Veröffentlichungsnachweis

Vor Upload müssen Quellrevision, Versionen, Prüfergebnisse, Paket-SHA256,
Softwarestückliste, Signier-/Updatestatus, Neuinstallation, Start, Modellverbindung,
Werkzeugfreigabe, Dateiimport/-download, Workflow, Stop, Neustart und
Wiederherstellung den tatsächlich angebotenen Bytes zugeordnet sein.
Fehlende Plattformabnahme bedeutet: diese Plattform noch nicht anbieten.
Produktionsbetrieb, vollständige Barrierefreiheit und jede externe Integration
werden durch diesen begrenzten Previewvertrag nicht nachträglich qualifiziert.
