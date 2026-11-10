# Private persönliche Geldbudgets

**Aktueller Paketstand:** [Installationslieferung vom 10. Oktober](INSTALLATION-DELIVERY-2026-10-10.md) sammelt die aktuellen nativen Paket-, Demo- und Serverbelege samt Grenzen.

Die lesende Status-API und Geldbudgetoberfläche sind inzwischen implementiert.
Der aktuelle optimierte Demo-Beleg (private development evidence; omitted from public sources)
bindet 1980 Eingaben, 202 Frontendtests und neun Browser-Prüfgruppen. Er prüft
exakte große Beträge mit sechs Nachkommastellen, unbekannte Kosten mit Betrag
null, fehlende Konfiguration, einen sichtbar veralteten Stand nach Abbruch,
Neustart und einen tatsächlichen Ollama-Lauf mit `qwen3:4b`. Der Release-Daemon
verwendet die korrigierten Autoritätsdateien; alle eigenen Prozesse sind positiv
beendet. Der Modelllauf ist keine Preis- oder Tokenizerparitätsabnahme.
Die darunter genannten Konfigurationsbelege bleiben auf ihren eigenen
archivierten Quellenständen gültig.

Der Daemon liest optional `COWORK_PERSONAL_MONEY_CONFIG_FILE`, bevor er Runtime,
Worker oder HTTP-Listener startet. Die Datei aktiviert dauerhafte gemeinsame
Installation-, Projekt- und Runbudgets. Neue Modellläufe werden bereits in ihrer
Erzeugungstransaktion an diese Konfiguration gebunden, vor `run_queued` und vor
jedem Workerclaim. Ein Workflow bindet alle vollständigen Providerprofile seiner
kompilierten Rollen, einschließlich unterschiedlicher Preise.

Die vollständige Konfigurationsprüfung (private development evidence; omitted from public sources)
qualifiziert 222 unveränderte Coreeingaben: Windows/Ubuntu jeweils 287 Rusttests
(26 ignoriert), acht/sieben Daemonbin-Tests, Format, Clippy und Daemonbuild.
PostgreSQL besteht 28 tatsächliche Fälle mit 25 beobachteten Provideranfragen.
Die eigenen nativen Jobs enden natürlich mit 109/7/7 Prozessen und null aktiv;
der temporäre PostgreSQL-Cluster ist positiv beendet. Alle 562 Archivmitglieder,
CRCs und damaligen Quellhashes wurden unabhängig geprüft. Quittungs-SHA256:
`09a60ed0588d52738a1bb8291c07be2eed31c0bdcb48cbb2a97abe8a8acfebd3`.
Der neuere vctip-Cleanupfehler bleibt als Fehlbeleg erhalten; die bestandene
Windows-Gesamtprüfung verwendet den warmen Cache. Linux nutzt einen eigenen
ext4-Buildpfad, einen Buildjob und deaktivierte Incrementaldaten. Der normale
Threadstack bleibt unverändert.

Status-API und Oberfläche sind im oben verlinkten Demo-Beleg separat geprüft.
Die nachfolgend genannten Belege qualifizieren ihren
archivierten Konfigurationsstand. Tatsächliche Modell-/Tokenizerparität,
Organisations-/Instanz-/Offlinebudgets und vollständige M2-Abnahme bleiben offen.

Die native Windows-Startprüfung (private development evidence; omitted from public sources)
besteht 15 tatsächliche Fälle auf den finalen 222 Coreeingaben. Zwei normale
Workerstarts, Neustart ohne Configenv, zwölf Startabweisungen und das echte
unveränderte Schema20-Release gegen Schema21 sind geprüft. Konfiguration,
Ledger und 65 Tabellenzeilenmengen bleiben beim Neustart gleich. Quittungs-SHA256:
`a40e4d88459b55c28665c1e23201c16e7782b97225826383e9283155ab8cd5a6`.
Die [Releasevorbereitung](DESKTOP-RELEASE.md) besteht separat unter Windows und
Ubuntu. Diese Belege sind keine Ausführungs- oder Installerabnahme der neuen
Releaseartefakte. Die vollständigen Rust-/PostgreSQL-Gates sind im oben verlinkten
Beleg gebunden.

## Datei und Autorität

Der Pfad muss absolut sein, darf kein `..` enthalten und muss eine gewöhnliche
Datei unmittelbar im kanonischen `COWORK_DATA_DIR` bezeichnen. Der gesamte
Datenordner ist vor den Lauf-Dateiwerkzeugen geschützt. Die Konfiguration darf
nicht im ausgelieferten Webordner liegen. Symlinks, Windows-Reparsepunkte und
Hardlinks sind abgewiesen. Maximalgröße: 1 MiB; leere Dateien sind ungültig.

Unter Windows muss der tatsächliche geöffnete Dateihandle dem aktuellen Benutzer
gehören und eine private ACL besitzen; SYSTEM und Administratoren sind zulässig.
Unter Linux muss der effektive Benutzer Eigentümer sein und Gruppen-/Andere-
Bits müssen fehlen, beispielsweise Modus `0600`. JSON-Fehler geben keine
eingelesenen Werte aus. Unbekannte Felder, doppelte Felder, falsche Formate und
unvollständige Providerprofile sind ungültig.

Browser-, Modell-, RunSpec- und Werkzeugpayloads erhalten keinen Schreibweg für
diese Betreiberkonfiguration. Persönliche Daemonautorität wird durch diesen
Schnitt nicht zu Organisationsautorität; Tenantprojekte werden abgewiesen.

## Konfigurationsformat 1

Dieses Beispiel beschreibt die Struktur. Beide SHA-Platzhalter müssen vor einer
Aktivierung durch die tatsächlich ausgewählten 64-stelligen Artefakt-/Quellhashes
ersetzt werden. Das Providerprofil muss vollständig mit dem ausgewählten Profil
des Laufs bzw. jeder Workflowrolle übereinstimmen. Die Beispielpreise sind keine
Tarifempfehlung oder aktuelle Anbieterpreise.

```json
{
  "format_version": 1,
  "installation": {
    "scope": { "kind": "installation", "id": "personal-installation" },
    "version": 1,
    "currency": { "code": "USD", "scale": 6 },
    "limit_units": 10000000
  },
  "project_limit_units": 5000000,
  "run_limit_units": 500000,
  "pool_policy_version": 1,
  "deployments": [
    {
      "deployment": {
        "id": "selected-local-model",
        "provider": {
          "id": "local-model",
          "name": "Selected local model",
          "kind": "ollama",
          "endpoint": "http://127.0.0.1:11434",
          "model": "selected-model",
          "revision": 1,
          "context_tokens": 32768,
          "credential_required": false,
          "credential_ready": true
        },
        "model_sha256": "<actual-model-artifact-sha256>",
        "price_id": "selected-price",
        "price_version": 1,
        "token_mode": "conservative_context_upper_bound"
      },
      "price": {
        "id": "selected-price",
        "version": 1,
        "provider_id": "local-model",
        "model_sha256": "<actual-model-artifact-sha256>",
        "currency": { "code": "USD", "scale": 6 },
        "input_units_per_million": 1500000,
        "output_units_per_million": 6000000,
        "cached_input_units_per_million": null,
        "source_sha256": "<actual-operator-price-source-sha256>"
      }
    }
  ]
}
```

Geld wird ausschließlich als geprüfte Ganzzahl verarbeitet. Mit `scale: 6` ist
eine Einheit ein Millionstel der Währung: `10000000` bedeutet 10 USD. Die
Preisfelder gelten pro Million Tokens. Die Berechnung rundet die Summe der
exakten rationalen Komponenten einmal nach oben. Reservierungen nehmen keinen
Cache-Rabatt an. Ein Limit von null ist zulässig und sperrt kostenpflichtige
Zulassung; es schaltet die Durchsetzung nicht ab.

Einträge benötigen eindeutige Deployment-IDs und vollständige Profile. Ein
Profil kann genau einem konfigurierten Deployment entsprechen. Deployment,
Preis und Installationspool müssen dieselbe Währung samt Skala haben. Modell-
und Preisversionen müssen zwischen Deployment und Preis exakt übereinstimmen.
Erlaubt sind HTTPS-Endpunkte sowie HTTP ausschließlich für die ausdrücklich
erkannten Loopbackhosts; URL-Credentials, Query und Fragment sind verboten.

`conservative_context_upper_bound` reserviert mit der vollständigen eingefrorenen
Kontextgrenze. `exact_artifact_renderer` wird abgewiesen, solange kein dafür
qualifizierter Runtimeadapter vorliegt. Die gespeicherten Modell-/Preishashes
binden die Betreiberentscheidung; dieser Schnitt beweist weder die tatsächlich
geladene Modellversion noch deren Tokenizer-/Rendererparität.

## Dauerhafte Bindung und Wiederöffnung

Aktivierung speichert Preise, Deployments, Installationspool, Ledger und zuletzt
den Konfigurationsdatensatz in einer gemeinsamen SQL-Transaktion. Die exakt
identische Konfiguration ist wiederholbar; eine Änderung von Limits, Profilen,
Preisen, Reihenfolge oder Versionen wird als Konflikt abgewiesen. Es gibt keine
stille Hot-Reload-, Reset- oder Abschaltoperation. Ein Neustart ohne die
Umgebungsvariable behält die bereits gespeicherte Konfiguration.

Zentrale Migration `0021` erstellt den Konfigurationsspeicher und sorgt dafür,
dass ältere Schema-20-Daemons die neue Datenbank vor Workerstart ablehnen. Sie
ändert keine ursprünglichen Ledger-, Operations- oder Legacy-Bindinghashes.

Neue Bindungen verwenden privates Format 2 und frieren RunSpec, Installation,
Projekt, Konfiguration und die vollständigen Roleprovider/Deployments ein.
Historische manuell gebundene Läufe behalten ihre ursprünglichen Bytes und
Quittungen. Alte ungebundene Läufe erhalten nach Aktivierung keine nachträgliche
Bindung und dürfen keine neue Modellanfrage senden. Native reine Dateirestores
benötigen keine Modellbindung. Unbekannte Modellwirkungen behalten ihre
ursprünglichen Reservierungen; Wiederöffnung oder Replay löst keinen HTTPretry
und keine zusätzliche Ausführungsautorität aus.

Nachweise und verbleibende Prüfungen stehen in
[CONTINUATION-2026-10-09.md](CONTINUATION-2026-10-09.md) und
[MODEL-ACCOUNTING.md](MODEL-ACCOUNTING.md).
