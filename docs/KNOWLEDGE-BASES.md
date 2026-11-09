# Wissensbasen, Projektordner und Agentenzugriff

Agenten können auf zwei Wissensbasen zugreifen:

- **Projektwissensbasis** – Infos, Notizen und importierte Dateien eines Projekts.
- **Persönliche Wissensbasis** – Einträge eines Nutzers, die projektübergreifend gelten.

Neue Läufe erhalten passende Einträge als Kontext (Texttreffer und aktuelle Themengraphen,
höchstens fünf Referenzen plus Profileinträge). Der gewählte Kontext wird mit dem Lauf
eingefroren; spätere Änderungen an Einträgen oder Schaltern verändern bereits
angenommene Läufe nicht. Inhalte und Schalter verleihen keine Lese-, Datei-, Werkzeug-
oder Freigaberechte.

## Projektoberfläche

Die Projektseite (Zahnrad neben „Projekte“) hat vier Bereiche:

| Bereich | Inhalt |
| --- | --- |
| Übersicht | Name, Projektanweisungen, Projektquellen, Standard-Arbeitsordner |
| Ordner | Verbundene Ordner des Projekts; im Desktop „Ordner auswählen“ (nativer Dialog, Standard nur lesen) und Zugriff entziehen |
| Wissensbasis | Agentenschalter, Notizen anlegen/bearbeiten/löschen, Dateien importieren (UTF-8, PDF, DOCX, XLSX, PPTX) |
| Archiv & Löschen | Archivierte Chats, Projekt löschen |

Verbundene Ordner stehen allen Chats des Projekts zur Auswahl. Ein Agent liest oder
ändert einen Ordner nur in Läufen, für die er ausgewählt wurde (siehe NATIVE-FOLDERS.md).
Im Browser bzw. bei Serververbindung können keine Rechnerordner registriert werden.

Importierte Dateien werden als Text extrahiert und in Abschnitten mit SHA256-Herkunft
gespeichert (siehe KNOWLEDGE-PROVENANCE.md). Die Originaldatei wird nicht als Datei
aufbewahrt.

„Meine Wissensbasis“ in der Seitenleiste verwaltet die persönliche Wissensbasis. Neue
Einträge werden als Memory mit Geltungsbereich `user` im immer vorhandenen Bereich
„Chats ohne Projekt“ gespeichert; bereits vorhandene kontoweite Memories aus anderen
Projekten erscheinen dort ebenfalls.

## Themenordner und Graph-RAG

Projektwissen und persönliches Wissen haben jeweils einen eigenen dauerhaften
Ordnerbaum. Unter „Themenordner“ einen Ordner anlegen; bei gewähltem Ordner entsteht
ein Unterordner. „Ordner bearbeiten“ benennt ihn um oder verschiebt ihn unter einen
anderen Ordner. Bis zu 500 Ordner mit zwölf Ebenen sind möglich. Zyklen und doppelte
Namen im selben übergeordneten Ordner werden abgewiesen.

Bestehende Einträge erscheinen zunächst unter „Ohne Ordner“. Über „Bearbeiten →
Themenordner“ lassen sich Notizen und importierte Abschnitte zuordnen oder verschieben.
Beim Import legt „Zielordner für den Import“ die gemeinsame Ablage aller Abschnitte
fest; Inhalt, Herkunft und Ordnerzuordnung werden atomar gespeichert. Leere Ordner
lassen sich löschen. Enthaltene Einträge oder Unterordner müssen vorher verschoben
werden. Suche und Ordnerfilter berücksichtigen bis zu 2.000 Einträge; der Schalter
„Unterordner einbeziehen“ grenzt die Ansicht und den Graphaufbau ein.

„Themengraph aufbauen“ speichert einen reproduzierbaren Graphen für genau den
gewählten Ordner, wahlweise einschließlich seiner Unterordner:

- Knoten: Themenordner, Dokumente/Abschnitte, Tags und zitierte Quellen.
- Verbindungen: Ordner enthält Eintrag/Unterordner, Eintrag trägt Tag, Eintrag zitiert
  Quelle. Gleiche Tags werden unabhängig von Groß-/Kleinschreibung verbunden;
  Quellknoten binden Identität, Revision und SHA256 zusammen.
- Graphsuche: Texttreffer bilden Startpunkte. Von den drei besten Startpunkten führt
  ein Schritt über gemeinsame Tags oder dieselbe Quelle zu weiteren Einträgen.
  Höchstens fünf Treffer samt Originaltext, Revision, Herkunft und Suchweg erscheinen.
  Ein gemeinsamer Ordner allein erzeugt keine inhaltliche Beziehung.
- Aktuelle Graphen ergänzen dieselbe Auswahl bei neuen Agentenläufen. Bestehende
  Zugriffsschalter und Projekt-/Nutzer-/Chat-/Agentengrenzen gelten weiterhin;
  der Kontext wird wie bisher beim Laufstart eingefroren. Ausdrücklich ausgewählte
  Wissenseinträge werden nicht durch weitere Graphtreffer erweitert.
- Inhaltliche Änderungen, Verschieben von Einträgen, Änderungen am gewählten
  Ordnerbaum oder hinzugefügte/gelöschte Einträge machen betroffene Graphen veraltet.
  Sie werden bis zum ausdrücklichen Neuaufbau nicht verwendet. Andere Themenbereiche
  machen einen unveränderten Graphen nicht veraltet.
- „Graph entfernen“ entfernt nur den abgeleiteten Graphen. Einträge bleiben erhalten.
  Maximal 32 Graphen pro Wissensbasis, jeweils 12.000 Knoten und 24.000 Verbindungen.

Dies ist ein Graph-RAG über strukturierte Metadaten mit lexikalischem Einstieg.
Automatische semantische Entitäts-/Relationsextraktion, Vektoreinbettungen,
Community-Zusammenfassungen und eine separate Graphdatenbank sind nicht enthalten.
Verbindungen sind Navigationshilfen, keine Belege für faktische Zusammenhänge.
Es erfolgen weder ein Modellaufruf noch ein externer Datentransfer beim Graphaufbau.
Die Teamprojekt-Dokumentverwaltung hat noch keinen eigenen Themenordnerpfad.

Ordner liegen in eigenen Tabellen neben den vorhandenen Dokumentrevisionen.
Projektarchive enthalten die Ordnerbäume und persönlichen Ordner; beim Import werden
Ordneridentitäten neu gebunden. Graphen werden als ableitbare Daten nicht importiert.
Ältere Archive ohne Ordner bleiben lesbar. Aktuelle Oberfläche und Backend gemeinsam
verwenden; ältere Programmstände kennen die neuen Dokumentfelder nicht.

Owner-Endpunkte (bestehende Bearer-/Origin-Prüfung, `personal=true` für persönliche Ordner):

| Endpunkt | Funktion |
| --- | --- |
| `GET/PUT /api/v2/projects/{project}/knowledge-folders` | Baum lesen/speichern, CAS über `expected_revision` |
| `GET/POST/DELETE /api/v2/projects/{project}/knowledge-graphs` | Graphen mit Aktualitätsstatus lesen, aufbauen, entfernen |
| `POST /api/v2/projects/{project}/knowledge-graphs/search` | Aktuellen Graphen abfragen |

`WriteKnowledge.location` enthält `folder_id` (null = ohne Ordner). Ohne `location`
erhält eine Bearbeitung die bisherige Zuordnung, damit bestehende Clients Einträge
nicht versehentlich aus Ordnern entfernen. Ungültige oder fremde Ordner werden auch
beim Import abgewiesen. Persönliche Ordner gelten kontoweit; Projektordner nur im
jeweiligen Projekt.

## Lokale Ausführung (Desktop und persönlicher Server)

Jeder Eigentümer stellt den Zugriff selbst ein:

- `GET/PUT /api/v2/knowledge-access` – Gesamtschalter für die persönliche Wissensbasis.
- `GET/PUT /api/v2/projects/{project}/knowledge-access` – pro Projekt
  `project_knowledge` und `personal_knowledge`.

Standard ist beides eingeschaltet (bisheriges Verhalten). Wirksam ist die persönliche
Wissensbasis nur, wenn Gesamtschalter **und** Projektschalter an sind. Schreibvorgänge
verwenden `expected_revision` (0 = Standardwerte); eine exakte Wiederholung nach
verlorener Antwort liefert dasselbe Ergebnis, sonst `revision_conflict`. Wird ein
Eintrag aus einer abgeschalteten Wissensbasis ausdrücklich ausgewählt, lehnt der Lauf
mit `knowledge_base_disabled` ab.

## Team-Server

Nur ein Plattformadministrator (globale Sitzung) liest und ändert die Richtlinie unter
„Konto & Sicherheit → Wissensbasen für Agenten“:

- **Private Projekte der Nutzer** – globaler Schalter oder Freischaltung einzelner Nutzer.
  Gilt für Projekt- und kontoweite Memories in privaten Serverprojekten.
- **Erstellte Teamprojekte** – globaler Schalter oder Freischaltung einzelner Projekte.
  Gilt für Wissensdokumente des Teamprojekts; Skills bleiben unberührt.
- **Persönliche Wissensbasis in Team-Läufen** – globaler Schalter oder Freischaltung
  einzelner Nutzer. Erlaubt der Person, die einen Team-Lauf startet, ihre persönliche
  Wissensbasis mitzugeben (siehe unten).

Standard auf dem Server ist **ausgeschaltet**. Ausnahmen wirken nur, solange der jeweilige
globale Schalter aus ist; ein Sperren einzelner Nutzer bei eingeschaltetem globalen
Schalter ist nicht vorgesehen.

| Endpunkt | Berechtigung |
| --- | --- |
| `GET/PUT /api/v2/teams/knowledge-access/policy` | Plattformadministrator |
| `GET /api/v2/teams/knowledge-access/me` | angemeldeter Nutzer (eigener Zugriff: `private_projects`, `personal_team_runs`) |
| `GET /api/v2/teams/{team}/projects/{project}/knowledge-access` | Leserecht am Teamprojekt |

Durchsetzung beim Laufstart: private Läufe (`android_wire_runtime_metadata`, Memory-Auswahl)
und Team-Läufe (`create_run_in_transaction`, Wissensdokumente und persönliche Wissensbasis)
lehnen bei fehlender Freigabe mit `knowledge_base_disabled` (HTTP 403) ab. Die Oberfläche
sendet in diesem Fall keine Wissens-IDs bzw. kein `personal_knowledge` und zeigt einen
Hinweis oder blendet die Option aus.

### Persönliche Wissensbasis in Team-Läufen

**Entscheidung: eigener Admin-Schalter, getrennt von „Erstellte Teamprojekte“ und „Private
Projekte“.** Teamwissen gehört ohnehin dem Team; persönliches Wissen dagegen gelangt über
die Ergebnisse zu allen, die das Teamprojekt lesen können. Das ist eine andere
Datenschutzentscheidung, deshalb schaltet weder `team_projects` noch `private_projects`
sie frei. Die Freigabe gilt pro Person (die Wissensbasis gehört ihr), nicht pro Projekt.

- Richtlinie: `personal_team_runs` (global) und `enabled_personal_team_users`
  (Ausnahmen). Ältere gespeicherte Richtlinien ohne diese Felder gelten als aus. Beim
  Schreiben sind beide Felder optional; fehlen sie, bleibt der gespeicherte Wert erhalten,
  damit ältere Clients den Schalter nicht versehentlich zurücksetzen.
- Anfrage: `TeamRunPlan.personal_knowledge = true` (Opt-in pro Lauf). Das Feld wird bei
  `false` nicht serialisiert, Anfrage-Hashes und Idempotenz früherer Läufe bleiben gleich.
- Inhalt: die kontoweiten Memories der startenden Person (`android_wire_sync_current`,
  `entity_type='memory'`, `team_id` und `project_id` leer, nicht gelöscht, nicht
  deaktiviert). Projektgebundene Memories und Einträge anderer Nutzer sind nie enthalten.
  Höchstens 20 Einträge, Schlüsselwort-Treffer zu Auftrag und Lauf-Prompt zuerst, je Eintrag
  höchstens 4.000 Zeichen.
- Einfrieren: Auswahl und Inhalt werden in derselben Transaktion wie der Laufstart im
  Laufkontext gespeichert (drittes Element von `team_runs.context`, nur wenn vorhanden),
  die Revisionen zusätzlich in der Ausführungsbindung. Spätere Änderungen an Einträgen oder
  an der Richtlinie ändern bereits angenommene Läufe nicht; eine exakte Wiederholung liefert
  die ursprüngliche Quittung.
- Kennzeichnung: Im Modellkontext stehen die Einträge als
  `<untrusted_personal_context id=… revision=… key=… category=…>` mit dem Hinweis, dass es
  persönliches Wissen der startenden Person ist, das Team die Ergebnisse sieht und die
  Einträge nur bei Bedarf zu verwenden sind. Wie Teamwissen verleihen sie keine Rechte.
- Oberfläche: Im Formular „Lauf starten“ erscheint die Option „Meine persönliche
  Wissensbasis einbeziehen“ nur, wenn `/me` `personal_team_runs` meldet. Sie ist pro Lauf
  standardmäßig aus und wird beim Wechsel von Team, Projekt oder Auftrag zurückgesetzt. Im
  Admin-Bereich hat der Schalter einen eigenen Abschnitt; meldet ein älterer Server die
  Felder nicht, wird der Abschnitt ausgeblendet und nichts davon gesendet.
- Geplante Läufe übernehmen das Feld aus ihrem gespeicherten Plan. Geprüft und eingefroren
  wird bei jeder Erzeugung für die Person, die den Zeitplan freigegeben hat.
- Kompatibilität: Ein älterer Server- oder Worker-Stand kann Läufe mit eingefrorenem
  persönlichen Wissen nicht lesen und bricht sie mit Konflikt ab. Oberfläche und Server
  sollten zusammen aktualisiert werden.

## Speicherung und Grenzen

- Eigenes, versioniertes Schema (`knowledge_access_schema_guard` Version 1) mit
  `native_knowledge_access` und `server_knowledge_access`, außerhalb der nummerierten
  Migrationen; ältere Programmstände ignorieren die Tabellen.
- Die Schalter sind nicht Teil von Projekt- oder Arbeitsbereichsexporten.
- Höchstens 1.000 Ausnahmen je Liste; die Verwaltung zeigt höchstens 500 Nutzer und
  500 Teamprojekte.
- Noch nicht umgesetzt: ein eigenes Suchwerkzeug für Agenten (Zugriff erfolgt über den
  eingefrorenen Kontext), eine Auswahl einzelner persönlicher Einträge für Team-Läufe und
  serverseitige Projektordner.

## Nachweise

- Themenordner und Graphsuche (04.11.2026): acht neue SQLite-Speicherfälle sowie
  elf bestehende Import-/Memory-/Zugriffsprüfungen bestehen. Sie prüfen unter
  anderem Zyklen, fremde Ordner, CAS/Wiederholung, atomaren Import, Quell- und
  Tagverbindungen, veraltete Graphen, Rechte-/Memorygrenzen, persönliche Graphen,
  deaktivierte Einträge, Neustart und Ordneridentitäten beim Archivimport.
- `services/daemon/tests/knowledge_topics.rs` besteht am aktuellen Windows-Dienst:
  Authentifizierung/Origin und der vollständige Ordner-/Graph-API-Weg.
- `tests/system/knowledge_topics_browser.mjs` besteht mit eigenem Windows-Dienst,
  privater Wegwerf-Datenbank und getrenntem Chromeprofil: 14 Prüfungen einschließlich
  erhaltenem Notizentwurf beim Ordnerwechsel, Import ins Unterthema, Tag-Nachbar,
  Veralten/Neuaufbau, Neuladen und 390-Pixel-Ansicht. Der gebundene Bericht und
  Screenshots liegen unter `.local/knowledge-topics-browser-20261104-final/`.
  763 generierte Verträge, Typprüfung und aktueller Web-/Windows-Dienstbuild bestehen.
  PostgreSQL, Linux, installierte Desktop-App und vollständige Barrierefreiheit
  wurden für diese Ergänzung nicht abgenommen.

- `crates/storage/tests/knowledge_access.rs`: lokale Schalter im tatsächlichen
  Modellkontext, ausdrückliche Auswahl, CAS/Wiederholung; Admin-only-Richtlinie,
  Ausnahmen, Validierung, globale Schalter; persönliche Wissensbasis in Team-Läufen
  (`team_runs_add_personal_knowledge_only_with_its_own_approval`): Ablehnung im
  Standard, trotz `team_projects`/`private_projects` und bei Ausnahme für eine andere
  Person; Freigabe per Ausnahme; Erhalt der Felder bei Schreibvorgängen ohne sie;
  Einfrieren gegenüber späteren Änderungen und Widerruf; Übergabe an den Executor nach dem
  Claim (über einen Test-Executor) und Kennzeichnung im Modellkontext
  (`knowledge_access::team_personal_context`, die `completion()` verwendet); keine
  deaktivierten, projektgebundenen oder fremden Einträge.
- `packages/ui/src/knowledgeAccessApi.test.ts`: strikte Antwortprüfung und Hilfsfunktionen,
  inklusive der optionalen Felder älterer Server.
