# Organisationen, Identität, Inhaltsrechte und Quoten

Stand: 2026-10-09. `cowork-identity` enthält ausführbare persistente Dienste und einen eigenen HTTP-Router. Die zentrale Migration, ein ausdrücklich aktivierter Daemon-Startup und separate Identity-/Tenant-Routen sind im Produkt integriert. Echte Tenant-Projekte werden in eigenen Tabellen gespeichert. Organisations-Aufträge, Runs und deren externe Effekte benötigen weitere Integration. Diese Teilumsetzung schließt M6 oder den gesamten Entwicklungsplan nicht ab.

## Persistenz und Identität

`IdentityStore::connect` unterstützt SQLite und PostgreSQL. `Store::identity()` stellt über `IdentityStore::from_pool(AnyPool, BackendKind)` denselben Produkt-Pool bereit. `Store::connect` registriert die Identity-, Accounting- und Tenant-Projektschemata in der bestehenden Migrationstransaktion; PostgreSQL behält den zentralen Advisory-Lock. Das Identity-Schema besitzt Version 1 und ausschließlich `identity_*`-Tabellen. Eine unbekannte Version wird vor weiteren Schemaänderungen abgewiesen. Vorhandene persönliche Projekte, Aufträge, Artefakte und SingleOwner-Daten werden nicht übernommen oder einer Organisation zugeordnet.

Der vertrauenswürdige Installer muss einmal `initialize_credentials` mit zwei verschiedenen Geheimnissen von jeweils mindestens 32 Bytes aufrufen: einem Bootstrap-Geheimnis und einem ausschließlich für den Ausführungsdienst bestimmten Abrechnungsgeheimnis. Persistiert werden getrennte SHA-256-Hashes mit unterschiedlichen Domänen. Bereits eingerichtete Hashes und die verbrauchte Bootstrap-Markierung können über diese API nicht zurückgesetzt werden. Das einmalige Bootstrap erzeugt einen lokalen Plattformadministrator und dessen erste Organisation mit Owner-Mitgliedschaft. Nach verlorenem Bootstrap-HTTP-Ergebnis erfolgt ein normaler Login; erneutes Bootstrap erzeugt keinen zweiten Administrator.

Lokale Konten besitzen kanonische ASCII-Benutzernamen und salted PBKDF2-HMAC-SHA256-Passworthashes mit 100.000 Iterationen. Passwörter werden weder gekürzt noch protokolliert. Eine Selbstregistrierung erhält keine Plattformrechte, Mitgliedschaften oder Inhaltsfreigaben. Externe Identitätsprovider, MFA und ein produktiver, verteilter Abuse-Schutz sind zusätzliche Integrations- und Abnahmearbeiten.

Die Session enthält zwei zufällige UUID-Komponenten und wird ausschließlich als Hash gespeichert. Die Laufzeit liegt zwischen einer Sekunde und 24 Stunden. Nur `authenticate_session` kann einen undurchsichtigen `Principal` erzeugen. Ein Benutzer-ID-String, der lokale SingleOwner-Daemon-Bearer sowie Bootstrap- und Abrechnungsgeheimnisse sind keine Tenant-Sessions. Logout widerruft die aktuelle Session. Passwortänderung benötigt zusätzlich das aktuelle Passwort, widerruft alle vorherigen Sessions und liefert eine neue Session. Ein Plattformadministrator kann alle Sessions eines Kontos widerrufen, auch die des letzten Owners. Kontodeaktivierung und Rollenänderung schützen den letzten aktiven Owner beziehungsweise Plattformadministrator; Session-Widerruf bleibt davon unabhängig verfügbar.

## Organisationen, Rollen und Inhaltsfreigaben

Organisationen sind `active`, `suspended` oder `closed`. Suspension und Wiederaktivierung erhöhen den Autorisierungsepoch; Schließen ist terminal. Ein Plattformadministrator verwaltet den Organisationszustand und Quoten, erhält dabei keine zusätzliche Inhaltsfreigabe.

| Rolle | Mitgliedschaften | Inhalte |
| --- | --- | --- |
| Owner | Alle Organisationsrollen verwalten, letzten aktiven Owner erhalten | Nur mit ausdrücklicher Inhalts-ACL |
| Admin | Member und Viewer verwalten; keine Owner-/Admin-Eskalation | Nur mit ausdrücklicher Inhalts-ACL |
| Member | Keine Mitgliederverwaltung | Eigene neue Ressourcen erzeugen; weitere Rechte nur mit ACL |
| Viewer | Keine Mitgliederverwaltung | Ausschließlich Read mit ACL |

Ressourcen besitzen genau eine Organisation und einen Typ: Projekt, Auftrag, Artefakt, Workflow oder Wissen. `create_resource` vergibt stets eine neue UUID und eine erste vollständige ACL für die aktuelle Mitgliedschaft des Erzeugers. Es existiert keine API, die eine beliebige vorhandene persönliche oder fremde Objekt-ID adoptiert. Die Resource-HTTP-Route erzeugt die Tenant-Ownership-Metadaten; sie erzeugt noch kein fachliches Projekt oder dessen Dateien.

Die ACL bindet Read, Write, Execute und Share an eine konkrete Mitgliedschaftsgeneration. Ohne Share-ACL darf auch ein Organisationsadministrator keine Freigabe lesen oder verändern. Neue Freigaben beginnen mit erwarteter Revision 0; Änderungen und Widerrufe benötigen die aktuelle Revision. Viewer dürfen ausschließlich Read erhalten. Nach Mitgliedschaftswiderruf erzeugt erneute Aufnahme eine neue Mitgliedschafts-ID; frühere Freigaben werden nicht übernommen. Auch Wiederholung einer früheren Resource-Erzeugung benötigt aktuellen Write-Zugriff auf das ursprüngliche Objekt.

`ContentPermit` bindet Identität, Session, Mitgliedschaft, Organisationsepoch, Objekt und Freigaberevision. Er ist weder frei konstruierbar noch aus HTTP-JSON deserialisierbar. Jede spätere Prüfung liest die persistierten Rechte erneut. Alte Permits werden nach Logout, Ablauf, Mitgliedschafts-/ACL-Widerruf oder Epochänderung abgewiesen. Das Audit verwendet einen persistenten monotonen Sequenzcursor, damit Einträge mit gleichem Zeitstempel bei Pagination nicht verloren gehen. Es enthält IDs und Aktionsnamen, keine Passwörter, Bearer-Tokens oder Dokumentinhalte.

## Transaktionsbindung für Produktdaten

Für ein neues Tenant-Projekt erzeugt `create_resource_in_transaction` in der Produkttransaktion die frische Objekt-ID samt Organisationsbindung und Creator-ACL. Die Anwendung schreibt anschließend ihren fachlichen Produktdatensatz mit genau dieser ID und Organisations-ID in derselben Transaktion. Ein Rollback verwirft Objekt, ACL und Idempotenzquittung gemeinsam. Die fachliche Payload muss zusätzlich unveränderlich an den Idempotenzschlüssel des Produktflusses gebunden werden; die reine Identity-Resource-Quittung bindet den Ressourcentyp.

`Store::create_tenant_project` setzt diesen Fluss um. Die eigene Tabelle `tenant_projects` speichert den vollständigen unveränderlichen Create-Request, dessen SHA-256, die ursprüngliche Produktquittung und die Organisationsbindung. Exakte Wiederholung liefert dasselbe Projekt; eine veränderte Payload mit gleichem Schlüssel kollidiert. Liste und Detail verwenden die aktuelle Mitgliedschaftsgeneration und Read-ACL innerhalb derselben Autorisierungstransaktion. Auch eine leere Liste prüft Session und Organisationsmitgliedschaft erneut. Tenant-Projekte werden nicht in `projects` eingefügt und erscheinen daher nicht in bisherigen SingleOwner-Listen. Ihre IDs sind fachliche Objekthandles; sie sind keine Hostpfade oder Dateisystemgrants.

Für einen bestehenden Datensatz gilt folgende Lockfolge:

1. Produkttransaktion auf demselben Pool öffnen und `authorize_content_in_transaction` aufrufen; dadurch wird zuerst der persistente Identity-Autorisierungslock genommen.
2. Fachliche Objekt-/Revisionlocks nehmen und den Datensatz zugleich nach Objekt-ID **und** Organisations-ID laden.
3. Nach jedem tatsächlichen Lockwait mit `validate_content_in_transaction` erneut prüfen; danach den Effekt in dieser Transaktion schreiben und committen.

Die Datenbankzeit wird nach dem tatsächlichen Lockwait neu gelesen: SQLite verwendet die aktuelle Datenbankzeit, PostgreSQL `clock_timestamp()`. Eine persistente UTC-Hochwassermarke weist Rücksprünge ab. Inhaltspolicies ersetzen keine bestehende Tool-, Dateisystem-, Netzwerk-, Approval-, Executor- oder globale Budgetpflicht. Ein `ContentPermit` allein berechtigt keinen externen Effekt. Die Anwendung muss die vorhandenen Gates zusätzlich im tatsächlichen Produktfluss erhalten und eine einheitliche Lockfolge mit diesen Gates integrieren.

## Quoten und Abrechnung

Quoten decken Model-Tokens, Kosten in ganzzahligen Mikrounits, Speicherbytes und gleichzeitig aktive Runs ab. Werte müssen nichtnegativ sein; Überläufe werden abgewiesen. Plattform- **und** Organisationsplan müssen ausdrücklich existieren, bevor eine Reservierung möglich ist. Ohne Plan wird abgewiesen. Eine Reservierung belastet beide Ebenen atomar; konkurrierende Organisationen können den globalen Plan nicht unabhängig überbuchen. Eine Planänderung benötigt ihre erwartete Revision und darf Held plus Consumed nicht unterschreiten.

Die Token-, Kosten- und Speicherwerte sind kumulierte Lifetime-Werte. Es gibt keinen impliziten Monatsreset, keine Rücksetzung beim Planwechsel und keine automatische Zeitablauf-Erstattung. Gleichzeitig aktive Runs werden reserviert und erst beim tatsächlichen Abschluss freigegeben. Die Anwendung muss die Reservierungsobergrenzen aus ihren autoritativen Modell-, Preis- und Laufzeitinformationen ableiten; diese internen Methoden sind keine untrusted Client-Abrechnungs-API.

`reserve_quota` benötigt ein gültiges Execute-ContentPermit und bindet den unveränderlichen Request an den Idempotenzschlüssel. Exakte Wiederholung liefert dieselbe ursprüngliche Prepared-Quittung, selbst nach Abrechnung; geänderte Wiederholung kollidiert. Die aktuelle persistierte Reservation verhindert dabei erneute Aktivierung. `activate_quota` prüft Session, ACL, Epoch und Frist erneut. `validate_execution` bestätigt vor weiterem Dispatch den aktiven persistierten Stand. Die Frist darf die ursprüngliche Sessionlaufzeit nicht überschreiten.

Für die gemeinsame Produkt-/Accountingtransaktion nimmt
`lock_quota_in_transaction` den bestehenden Installationlock auf exakt der
geborgten Callerconnection. Der nicht konstruierbare, nicht klonbare und nicht
serialisierbare `LockedIdentityQuota` bietet Reserve, Activate und Finalize als
`*_in_transaction`-Methoden sowie Inhalts- und Execution-Revalidierung. Seine
`transaction()`-Methode stellt dieselbe SQLtransaktion für anschließende Ledger-
und Produktlocks bereit. Die Reihenfolge lautet Identity zuerst, danach
Accounting und Produkt-/Runlocks. Nach tatsächlichen weiteren Lockwartezeiten
ist die Autorisierung mit frisch gelesener Datenbankzeit erneut zu prüfen.

Mutationen benutzen Savepoints auf derselben Connection; sie öffnen keine zweite
Pooltransaktion und committen niemals die äußere Transaktion. Insbesondere wird
eine bereits geschriebene Plattformbelastung zurückgerollt, wenn anschließend
die Organisationsquote fehlschlägt, auch wenn der Caller diesen Fehler abfängt
und andere Produktdaten committen möchte. Bei nicht bestätigtem Savepointrelease
oder -rollback beziehungsweise Abbruch des Mutationsfutures wird der Guard
vergiftet und verweigert sämtliche APIs einschließlich `transaction()` mit
`TransactionPoisoned`. Der Caller muss dann die äußere Transaktion zurückrollen.
Zurückgegebene Quittungen und Metadaten sind bis zu deren Commit vorläufig und
erteilen keine externe Dispatchberechtigung. Die bisherigen Standalone-Methoden
sind kompatible Wrapper mit eigenem abschließendem Commit.

`concurrent_runs == 1` bleibt eine Run-Lifecycle-Reservierung. Diese API darf
nicht unverändert pro Modellversuch eingesetzt werden; die getrennte Token-/Geld-
und Attemptintegration benötigt weiterhin ihre eigenen gemeinsamen Zulassungs-
und Abschlussgrenzen.

Eine aktivierte Reservation bleibt bei Timeout, Logout, Widerruf, Stop-Anforderung oder Quarantäne vollständig Held. Nur der getrennt authentifizierte vertrauenswürdige Executor kann `finalize_quota` aufrufen. Er muss zuvor den tatsächlichen Ausführungsabschluss positiv verifizieren. Der Identity-Dienst bindet dessen Abschluss-SHA-256 und den tatsächlichen Verbrauch atomar an beide Quoten; er prüft selbst keine Betriebssystem-, Provider- oder Container-Abschlussbelege. Vor Dispatch darf der vertrauenswürdige Dienst eine noch Prepared-Reservation mit Nullverbrauch abschließen. Nach Aktivierung ist diese Abkürzung ausgeschlossen. Exakte Abschlusswiederholung ist idempotent; geänderte Verbrauchs- oder Belegwiederholung wird abgewiesen. Persistierte Originalquittung und Requesthash schützen gegen widersprüchliche oder beschädigte Reservation-Payloads.

## HTTP-Oberfläche

`cowork_identity::http::router(store)` liefert einen eigenständigen Router für explizites Nesting unter `/api/v2/identity`. Er besitzt Bootstrap, Registrierung, Login, Logout, eigene Identität und Passwortwechsel, Organisations-/Mitgliedschaftsverwaltung, Resource-Metadaten und ACLs, Organisations-/Plattformquotenpläne, Kontodeaktivierung, Session-Widerruf und scoped Audit. Reservierung, Aktivierung und Executor-Abrechnung besitzen hier keine öffentlichen Endpunkte. Bestehende SingleOwner-Endpunkte werden durch den Router nicht tenantfähig.

`router_with_identity` hängt Identity und den separaten Tenant-Router außerhalb des bisherigen SingleOwner-Bearer-Layers ein. Unter `/api/v2/tenant/organizations/{org}/projects` stehen GET und POST für echte Tenant-Projekte zur Verfügung; `/api/v2/tenant/organizations/{org}/projects/{project}` liefert ein autorisiertes Detail. Create akzeptiert ausschließlich `name` und `idempotency_key`; Objekt-ID, Organisations-ID im Body und Hostpfade werden abgewiesen. Der Tenant-Router besitzt ein eigenes Bodylimit von 8 KiB und eine Begrenzung von 600 Requests pro Minute.

Der Daemon aktiviert diese Oberflächen nur mit `COWORK_ENABLE_TENANT_IDENTITY=1` und ausdrücklich konfigurierten `COWORK_IDENTITY_BOOTSTRAP_SECRET_FILE` sowie `COWORK_IDENTITY_SETTLEMENT_SECRET_FILE`. Die Dateien dürfen nicht im ausgelieferten Webverzeichnis liegen und enthalten 64 bis 128 Hexzeichen. Private Dateiberechtigungen und die sichere Abgrenzung der Installationsgeheimnisse werden zusätzlich im Startup geprüft und qualifiziert; eine produktive Mehrbenutzer-/Deployment-Abnahme ist damit noch nicht erteilt.

DTOs weisen unbekannte Felder ab. Die Oberfläche begrenzt JSON-Bodies auf 32 KiB und den Routerprozess auf 600 Requests pro Minute, davon höchstens 20 Credential-Requests. Weiterleitungs-IP-Header umgehen diese Grenzen nicht. Ein verteilter Betrieb benötigt zusätzlich einen gemeinsamen Edge-Limiter. Autorisierung akzeptiert genau einen Bearer-Header. Authentifizierungsfehler und Zugriffsfehler haben feste JSON-Antworten; fremde und fehlende scoped Objekte werden gleich behandelt. Datenbankfehler, Passwörter und Token erscheinen nicht in Fehlerantworten. Antworten werden mit `Cache-Control: no-store` versehen.

## Verifikation und verbleibende Abnahme

Windows: `cargo test -p cowork-identity --offline --locked` besteht mit **13 SQLite-Persistenztests und 3 HTTP-Tests**. Geprüft werden Wiederöffnung, Bootstrap-Einmaligkeit, Credentials-/Session-Trennung, Passwort-/Session-Widerruf, Rollen und Inhaltsisolation, Mitgliedschaftsgenerationen, Suspension/Epoch, globale Quoten über konkurrierende Organisationen, atomarer Rollback, unveränderliche Quittungen, manipulierte Payloads, UTC-Rücksprung, Auditpagination, strikte HTTP-Felder sowie Body- und Rate-Limits.

Der zusätzliche Test `postgres_persistent_authorization_and_quota_contract` ist standardmäßig ausdrücklich ignored. Seine gesonderte Ausführung benötigt `COWORK_IDENTITY_TEST_POSTGRES_URL` auf eine frische private Fixture-Datenbank und prüft vorab, dass keine `identity_*`-Tabellen existieren. Ein fehlender PostgreSQL-Lauf gilt nicht als bestandene PostgreSQL-Abnahme.

Die zusätzliche aktuelle Callertransaktionsprüfung besteht unter Windows mit
sechs neuen SQLiteverträgen; der vollständige Identity-Cratelauf besteht mit
22 Tests und vier ausdrücklich ignorierten PostgreSQLverträgen sowie Format
und Clippy. Drei neue PostgreSQLverträge wurden tatsächlich in einer eigenen
frischen Loopbackinstanz ausgeführt. Sie prüfen gemeinsamen Outerrollback,
abgefangenen Organisationsfehler ohne Plattformbelastung, Activation-/Settlement-
Rollback, unveränderliche Replays, echte Konkurrenz zwischen Organisationen,
Sessionablauf nach Installationlockwait und nach späterem echtem Produktrowwait.
Die private SQLitefehlerprobe bestätigt den vergifteten Guard nach fehlgeschlagenem
Savepointrollback. Native Prüfung (private development evidence; omitted from public sources)
und PostgreSQLprüfung (private development evidence; omitted from public sources)
binden jeweils zwölf unveränderte Inputs. Eigener Postmaster, Fixture und nativer
WSL-Launcher haben positive Exit-/Cleanupbelege. Dies ist die TX-Grundlage und
keine bereits integrierte Organisationslauf- oder Modellabrechnungsabnahme.

Die abgegrenzten Windows-Läufe `cargo test -p cowork-storage tenant_projects::tests --offline --locked` und `cargo test -p cowork-daemon tenant_routes::tests --offline --locked` bestehen mit **vier Tenant-Storage- und drei Tenant-HTTP-Tests**. Sie prüfen echte Projektpersistenz und Wiederöffnung, vollständige Payload-Idempotenz, Legacy-Trennung, atomaren Rollback bei fehlgeschlagenem Produktinsert, aktuelle ACLs, Widerruf und neue Mitgliedschaftsgenerationen, leere autorisierte Listen, fremde Organisationen, strikte Bodyfelder und Hostpfad-Abweisung. Der zusätzliche `postgres_tenant_projects_contract` verlangt `COWORK_TENANT_PROJECT_TEST_POSTGRES_URL` auf eine eigene, vollständig neue private PostgreSQL-Datenbank. Die gesonderte aktuelle PostgreSQL-Qualifikation wird im entsprechenden Abschlussbeleg ausgewiesen.

Ausstehend bleiben Org-Aufträge und Runs samt bestehender globaler Tool-/Policy-/Executor-/Accounting-Gates, die UI, externe Auth-Provider und die breite M6-/Deployment-Abnahme. Der neue Projektfluss führt keine Tools oder externen Effekte aus. Historische M5-Belege und Quellenarchive werden durch diese Weiterentwicklung nicht umgeschrieben.
