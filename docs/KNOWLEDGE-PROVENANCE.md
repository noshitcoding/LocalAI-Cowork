# Herkunft von Wissen und Memory

Persönliche Wissens-/Memoryeinträge und Teamdokumente tragen eine versionierte
Herkunft. Neue manuelle Einträge sind `user_authored`; importierte Inhalte sind
`imported`, aus Quellen übernommene Inhalte können ausdrücklich als `retrieved`
erfasst werden. Vorhandene Einträge ohne Quellen bleiben `legacy_unknown`.
Die Migration erfindet keine frühere Herkunft und keine verlorenen Revisionen.

Jede Quelle bindet Formatversion 1, eine URI, Datei-ID oder Artefakt-ID, die
Quellenversion, einen SHA256-Wert und eine konkrete Fundstelle. Bei Datei- und
Artefaktquellen prüft der Server den gespeicherten Snapshot im selben persönlichen
bzw. Teamprojekt. Team-Dateien verwenden ihre unveränderliche Blobversion;
Geräteartefakte verwenden ihre konkrete Revision. Eine fremde Projekt-ID oder
ein anderer Hash wird abgewiesen. Die Herkunft erweitert keine Leserechte.

Die Prüfung `snapshot_verified` vergibt ausschließlich der Server. URI-Quellen
bleiben `declared`: der Hash und die Fundstelle sind die ausdrückliche
Quellenangabe des Autors. Der Server ruft eine URI durch das Speichern nicht ab.

Dokumentinhalt, Herkunft und Quellen werden gemeinsam mit jeder neuen Revision
als unveränderlicher Datensatz gespeichert. Die UI kann die tatsächlichen
Revisionen, deren Inhaltshash und Herkunft anzeigen. Die geschützten Endpunkte
sind `/projects/{project}/knowledge/{id}/revisions` und
`/teams/{team}/projects/{project}/documents/{id}/revisions`, mit `after` und
`limit` (höchstens 100). Teamzugriffe prüfen die aktuelle Sitzung und Projekt-ACL.

Persönliche V2-Exporte nehmen Quellen und gespeicherte Revisionen mit. Ein Import
erhält die alten Inhalte mit neuen lokalen Revisionsidentitäten und markiert die
Importquelle. Datei-/Artefaktverweise aus dem Quellprojekt werden inert zu
Archiv-URIs; sie werden nie in neue Dateizugriffsrechte übersetzt. V1-Definitionen
verweisen auf den Hash ihrer tatsächlich übernommenen Quellkonfiguration.

Neue Modellläufe frieren den ausgewählten Inhalt samt Herkunft in ihrem Kontext
ein. Eine spätere Änderung an einem Dokument ersetzt keine bereits gespeicherten
Modellanfragen oder historischen Antworten. Eine Fundstelle ist Kontext und
erteilt keine Tool-, Dateizugriffs- oder Ausführungsfreigabe.
