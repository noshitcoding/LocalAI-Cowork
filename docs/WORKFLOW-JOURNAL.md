# Dauerhafte Workflowentwuerfe und Vorgangsbelege

**Scoped workflow questions qualification passed (2026-10-09):** The single-owner input slice is source-bound in the immutable input evidence (private development evidence; omitted from public sources): seven reports, 263 inputs; Windows 364/Ubuntu 347 Rust tests with format/Clippy, 11 PostgreSQL contracts, ten daemon process suites per platform, 145 frontend tests on 71 sources/six Dist files and 20 browser checks on 74 sources. Current versions: database 19, event schema 7, export 12, 161 generated contracts. input_resume_available remains false. Historical 144-contract, 137-/102-test and 18/7/11 proofs keep their original scope. Native GUI, WCAG/NVDA, enterprise, complete M5/M0-M10 and release acceptance remain false. The 1440px conversation layout still needs improvement; see [WORKFLOW-INPUTS.md](WORKFLOW-INPUTS.md).

Dieser private V2-Produktschnitt erweitert die persoenliche Ablaufbibliothek und
Workflowstarts um einen dauerhaften Anwendungsdienst. Teamrechte, Synchronisation
und Offline-Ausfuehrung sind damit noch nicht implementiert. SQLite und PostgreSQL
verwenden denselben fachlichen Vertrag. Migration 18 fuehrte dieses Editor-/
Vorgangsjournal ein; der aktuelle Datenbankstand ist 19. Die getrennten
[Agentenrueckfragen](WORKFLOW-INPUTS.md) besitzen eigene Antwortentwuerfe, Snapshots
und FrozenIntents. Ihr Commit ist strikt bodylos; die unten beschriebenen
Commit/Discard/Acknowledge-Routen des Bibliotheksjournals bleiben davon getrennt.

## Entwuerfe und Konflikte

Der Editor speichert auch unvollstaendige Formulardaten als eigenes Dokument mit
Formatkennung `cowork.workflow-editor.v1`. Ein Entwurf ist keine ausfuehrbare
Workflowdefinition und erteilt keine Rechte. Erst eine getrennte Publikation wird
fachlich validiert und erzeugt eine unveraenderliche Workflowversion.

Jede bestaetigte Entwurfsrevision verweist auf einen unveraenderlichen Snapshot.
Eine Speicheraenderung traegt Basisrevision und Idempotenzschluessel. Ein verlorener
Erfolg wird mit demselben eingefrorenen Request abgeglichen; die aktuelle Revision
ersetzt die damalige Antwort nicht. Derselbe Schluessel mit anderem Inhalt wird
verweigert.

Eine veraltete Basis erzeugt HTTP 409 und eine dauerhaft gespeicherte Konfliktkopie.
Die Antwort enthaelt den damals bestaetigten Head und den eingehenden Snapshot.
Spaetere Revisionen veraendern diesen Beleg nicht. Beide Snapshots muessen derselben
historischen Konfliktrevision angehoeren. Der Editor verlangt eine bewusste neue
Basis; er ueberschreibt den bestaetigten Stand nicht im Hintergrund.

Unbekannte oder beschaedigte Editorformate bleiben zur Pruefung und zum Export
erhalten. Sie werden nicht automatisch als leeres Formular ersetzt. Kollidierende
Rollenumbenennungen werden sichtbar abgewiesen. Beim Reparieren bereits
mehrdeutiger Rollennamen verlangen betroffene Schritte eine ausdrueckliche neue
Zuordnung.

## Vorbereitete Vorgangsabsichten

Erstellen, Veroeffentlichen, Sperren und Workflowstart verwenden drei Phasen:

1. `prepare` speichert die vollstaendige urspruengliche Absicht und optional den
   bestaetigten Entwurfsnapshot. Dabei entsteht noch keine fachliche Wirkung.
2. `commit` verarbeitet genau diese gespeicherte Absicht. Bei Starts prueft die
   Runtime Modellverfuegbarkeit ausserhalb der Datenbanktransaktion und bindet die
   letzte Zulassung innerhalb derselben Transaktion wie Lauf und Vorgangsbeleg.
3. `acknowledge` bestaetigt einen terminalen Beleg fuer die Bedienoberflaeche.
   Historie und Idempotenzsperren bleiben bestehen.

Die Zustaende sind `pending`, `accepted`, `rejected` und `discarded`. Abgleichen,
Wiederholen, Aufgeben und Bestaetigen verwenden ausschliesslich die gespeicherte
Vorgangs-ID. Die Aktionsrouten nehmen keine neue Laufkonfiguration entgegen.
Die Oberflaeche erzeugt nach einem unklaren Transportausgang keinen Ersatzschluessel.
Vor einem neuen Vorgang laedt sie die offenen Belege im jeweiligen Projekt und
Auftragskontext. Ein Kontextwechsel uebernimmt keine Aktivitaet des alten Clients.

`discard` ist eine transaktionale Sperre gegen eine noch ausstehende Wirkung. Falls
der Vorgang schon angenommen wurde, liefert es den angenommenen Beleg; ein
angenommener Lauf wird dadurch nicht abgebrochen. Laufabbruch bleibt eine eigene
Operation. Parallel eintreffende direkte Alt-API-Aufrufe verwenden dieselben
Schreibschluesselsperren und koennen aufgegebene oder abgelehnte Absichten nicht
umgehen.

Transiente Provider-, Speicher- und unklare Transportfehler erhalten `pending`.
Nur eindeutig klassifizierte fachliche Ablehnungen werden dauerhaft `rejected`.
Historische angenommene Antworten sind auch nach Profilentzug oder einer neueren
Workflowversion lesbar; neue Aktionen durchlaufen weiterhin die aktuelle Policy.
Ein angenommener Startbeleg enthaelt die urspruengliche eingefrorene Konfiguration,
keinen nachtraeglich rekonstruierten aktuellen Laufzustand.

## Schnittstellen unter `/api/v2/projects/{project}`

| Pfad | Methoden und Verhalten |
| --- | --- |
| `/workflow-drafts` | GET paginierte Heads; PUT mit `SaveWorkflowDraft` |
| `/workflow-drafts/{draft}` | GET bestaetigter Head |
| `/workflow-drafts/{draft}/snapshots` | GET paginierte unveraenderliche Snapshotmetadaten |
| `/workflow-drafts/{draft}/snapshots/{snapshot}` | GET autorisierter Snapshot |
| `/workflow-operations` | GET paginierte unbestaetigte Belege; POST `PrepareWorkflowOperation` |
| `/workflow-operations/{operation}` | GET gespeicherte Absicht und Zustand |
| `/workflow-operations/{operation}/commit` | POST gespeicherte Absicht verarbeiten |
| `/workflow-operations/{operation}/discard` | POST ausstehende Wirkung sperren |
| `/workflow-operations/{operation}/acknowledge` | POST terminalen Beleg bestaetigen |

Aktionsbodies sind leer oder `{}`. Pagination verwendet Offset und Limit; eine
bestaetigte Historie kann ausdruecklich mit `include_acknowledged=true` gelesen
werden. Belege muessen zum angefragten Projekt passen; diese Pruefung ersetzt
keine Teamautorisierung. Rust ist die Vertragsquelle
fuer die aktuell 161 generierten Schemas, OpenAPI- und TypeScriptdefinitionen.
Die historischen Journal-/Snapshotnachweise banden 144 Definitionen. Aktuelle
Vertragsversionen: Datenbank 19, Ereignisschema 7, Export 12; die neuen optionalen
Antwortarchive aktivieren weder Rechte noch historische Pending-Intents.

## Speichergrenzen und Betrieb

Pro Projekt sind hoechstens 32 Entwurfsheads, 32 MiB serialisierte Snapshotdaten
und 512 Vorgangsbelege zugelassen. Ein Entwurf besitzt hoechstens 128 Snapshots;
sein Inhaltsrequest ist auf 1.000.000 serialisierte Bytes begrenzt. Konfliktkopien,
aufgegebene und bestaetigte Belege zaehlen weiter. Kompakte interne
Speicherquittungen verweisen auf unveraenderliche Snapshots und vervielfachen nicht
jeden grossen Head bei kleinen Konflikten.

Es gibt keine automatische Bereinigung oder Kompaktierung. Bei erreichter Grenze
bleibt der letzte bestaetigte Stand bestehen; unbestaetigte Editordaten koennen
bewusst exportiert oder verworfen werden. Export nutzt denselben geprueften
Downloadpfad wie andere Ergebnisse, einschliesslich Desktop-Bridge und sichtbaren
Abbruch-/Fehlerzustaenden. Interaktive Betriebssystem-Speicherdialoge bleiben eine
separate offene Abnahme.

## Nachweise und Grenzen

Die abgeschlossene historische Journalqualification wurde in
workflow-journal-2026-10-09.json (private development evidence; omitted from public sources)
zusammengefuehrt. Sie bindet den damaligen Quellenstand fuer Windows-/Ubuntu-Kern und
neun PostgreSQL-Vertraege sowie fuer neun Daemon-Prozesssuiten je Plattform. Der
Frontend- und Browserbericht besitzen eigene vollstaendige Quellen-/Binarybelege.
Vorversuche mit spaeter korrigierten Quellen bleiben erhalten.

Der fruehere Workflow-Produktbeleg (private development evidence; omitted from public sources)
enthaelt die damaligen Desktop-Build-/Lebenszykluspruefungen. Er qualifiziert nicht
automatisch den neuen Journalquellenstand oder neue Desktop-GUI-Interaktionen.

Ein operationsgebundener alter Editordatenstand kann ausdruecklich geprueft und
als unabhaengiger neuer Entwurf kopiert werden. Snapshot, Originalversion und
Vorgangsbindung werden geprueft; der aktuelle Head ersetzt die alte Quelle nicht.
Die Bestaetigung des alten Vorgangs behaelt die neue Kopie. Eine neue
Publikationsbasis wird ausschliesslich nach sichtbarer ausdruecklicher Auswahl
uebernommen.

Der getrennte Snapshotnachweis (private development evidence; omitted from public sources)
enthaelt 102 Frontendtests, Typpruefung, Vertragspruefung, Build und 14 gezielte
Browserpruefungen sowie erneut gepruefte unveraenderte Kern-/Prozessberichte: sieben
Berichte, 249 Quellen. Der Journalnachweis bleibt unveraendert historisch erhalten.
Nach einem geschlossenen Browser-Testkontext wurde der Kernablauf erneut geprueft;
die tatsaechlichen Exportbytes wurden validiert, die Browser-Dateiablage bleibt
unqualifiziert. Diese Nachweise erteilen keine native Desktop-GUI-Abnahme.

Der [Rueckfragen-Schnitt](WORKFLOW-INPUTS.md) ist inzwischen implementiert: je
Agent gebundene Fragen, CAS-Antwortentwuerfe, unveraenderliche Konfliktsnapshots,
eingefrorene Prepare-Absichten und atomare AnswerReceipt/Event/Intent-Commits.
Accepted-Historie bleibt abgleichbar; neue Aktionen pruefen aktuelle Lease,
Policy, Widerruf, Frist und dauerhaft bekannte Abbruchprovenienz. Der Parent
bleibt waehrend Fragen Running; Geschwister koennen weiterarbeiten.
input_resume_available bleibt false. Der aktuelle Input-Sammelbeleg (private development evidence; omitted from public sources)
besteht sieben Berichte/263 Quellen: Windows 364/Ubuntu 347 Rust+Format/Clippy,
elf PostgreSQL-Verträge, zehn Prozesssuiten je Plattform, Frontend 145/71/sechs Distdateien
und Browser 20/74. Der fruehere 137-Frontendbeleg bleibt historisch erhalten. Die
1440px-Unterhaltungsflaeche ist durch Startbeleg und gesperrten Composer zu niedrig;
allgemeine UI-/native GUI-/WCAG-/Enterprise-/M5-Abnahme bleibt offen.

Einzelagentenstarts behalten ihren bestehenden eingefrorenen Wiederholungspfad;
noch nicht jeder Produktbefehl verwendet dieses neue Journal. Externe
Workflowwerkzeuge, Hierarchie, Input-Wiederaufnahme, allgemeiner belegter Resume, Skills,
Wissen/Memory und Zeitplanung fehlen weiter. Synthetische lokale Provider ersetzen
keine reale Modellhardware-, Enterprise-, Compose-, Kubernetes-, Last- oder
Barrierefreiheitsabnahme. M5 und der vollstaendige Neubau bleiben offen; produktive
Umstellung und Release sind nicht freigegeben.
