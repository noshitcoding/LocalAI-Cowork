# Dokument- und Medienimplementierung

Arbeitsstand vom **24.10.2026**, V2 **2.0.0-alpha.7**. Dieser Stand setzt den
vereinbarten erweiterten Plan um: eigene Fachansichten, OCR, lokale und ausdrücklich
gewählte Cloud-Bildmodelle sowie alle V2-Ausführungsziele. Der Gesamtplan ist noch
**nicht vollständig implementiert**. Der frühere Audit vom 20.10. bleibt ein historischer
Quellen- und Testbeleg; dessen Ausschluss der Erweiterungen ist kein aktuelles Scope-Limit.

## Implementierter gemeinsamer Unterbau

- Der Workspace-Abgleich erlaubt eingeschränkte Nur-Lesen-Grants, ohne Identität,
  Revision oder Schreibrechte auszuweiten. Der zuvor ignorierte Test ist aktiv; ein
  zusätzlicher echter Dokumentleselauf prüft die erfolgreiche Nutzung.
- `document-actions/2` bietet dauerhafte Vorbereitung, Wiederabruf nach verlorenem
  Response und idempotenten Start. Quellen, SHA, Ausgabe und Engine-Auswahl werden
  vor der Prüfung gebunden. Der Start verwendet keinen Modellprovider und läuft durch
  die vorhandenen Freigaben und das Generate-Effektjournal.
- Die Laufzeit hält eine Programmauswahl über alle Phasen fest. Ein Wechsel zwischen
  Prüfung und Start wird abgelehnt. Quellenkonflikte und bestehende Ziele werden geprüft.
- Neue Entwürfe liegen serverseitig mit CAS-Revision, unveränderlichen Snapshots und
  gespeicherten Konfliktkopien. Ein wiederholtes Speichern mit derselben Kennung liefert
  die ursprüngliche Antwort. Migration **0100** unterstützt SQLite und PostgreSQL.
- Die neue Workbench verwendet denselben Dokumentpfad wie der lokale Chat. Sie bietet
  Dateiliste, verifizierte Downloads, Erstellung von DOCX/XLSX/PPTX/PDF, TeX-Quelltext
  über den bestehenden Importpfad, grundlegende Office-Inhaltsänderungen, PDF-Seiten,
  PDF-Formularfelder, Bildschritte, TeX-Kompilierung sowie Vorschauen und Diagnosen.
- Der ursprüngliche native Office-Pfad bleibt für bestehende Läufe verfügbar. Die neue
  Workbench benötigt keine Benutzerrechte-Freigabe und keine Modellkonfiguration.
- Entwurfsverlauf und Konfliktkopien können geladen werden. Die lokale Aktionsreferenz
  enthält nur Kennung und Phase. Ein unbestätigter Start wird wieder abgeglichen.
- Bereitschaftsgründe sind getrennt sichtbar. Die Workbench wartet auf die Engine-Prüfung;
  Office-Konvertierung wird durch eine echte Probe bestätigt. Nicht darstellbare Bildformate
  bieten einen Hinweis auf die gerenderte PNG-Vorschau. TeX/BibTeX-Dateien werden erkannt.
  Ein Konfigurations-Reload prüft auch unveränderte Programmpfade erneut, damit eine
  reparierte Wheel-Installation oder ein erneuerter Cache nicht dauerhaft als defekt gilt.
- XLSX-Inspektion und Erzeugung kennzeichnen Formelcaches als ungeprüft. Eine ausdrücklich
  gewählte XLSX-Neuberechnung entfernt alte Ergebniswerte aus einer gebundenen privaten
  Kopie und läuft über LibreOffice. Der Cache-/Fehlerbericht zählt auch die entfernten
  Quellcaches. Externe Beziehungen und Netzwerkformeln werden vorher abgelehnt.
  Das ist noch keine Abnahme vollständiger Excel-Kompatibilität.
- Rechenstatus und Engine-Diagnosen stehen direkt in der Ergebnisansicht. Fehlerwerte,
  fehlende Ergebniswerte und ungeprüfte Caches werden getrennt erklärt; eine erzeugte
  Arbeitsmappe mit Rechenfehlern wird dadurch nicht als fehlerfrei dargestellt.
- Excel-Strukturänderungen aktualisieren unterstützte A1-/absolute und Blattreferenzen,
  ganze Zeilen/Spalten sowie globale und lokale Namen. Löschungen verkleinern Bereiche
  oder erzeugen `#REF!`. Komplexe Strukturen werden gezielt vor Veröffentlichung abgelehnt.
  Einfache Diagramme/Bilder bleiben bei Zelländerungen erhalten; Paket-/Objekt-/Bildprüfungen
  gelten auch für Vorlagen. Sparse-Verschiebungen haben eine feste Zellgrenze.
- Die Bildansicht bietet alle 19 bestehenden Operationen mit eigenen Parameterfeldern,
  bis zu 50 Schritten, Umordnen, Entfernen und Rückgängig/Wiederholen. Versionierte
  Bildentwürfe werden über den gemeinsamen Dienst gespeichert. Quellenwechsel erfordern
  einen neuen Bildentwurf; EXIF-Erhalt ist eine ausdrückliche Option.
- Die LaTeX-Ansicht erfasst ein versioniertes Projektmanifest mit Hauptdatei,
  Begleitdateien und festen SHA-256-Werten. Hauptdateiwechsel, gespeicherte Entwürfe
  und Quellenansicht sind verbunden. Passende Diagnosen öffnen die gebundene Datei
  an ihrer Zeile; geänderte Quellen werden beim Öffnen und Prüfen zurückgewiesen.
  Das bestehende Limit beträgt weiterhin 16 Dateien.
  Der TeX-Logparser erkennt auch ohne `.tex` protokollierte `\input`-Dateien und
  bevorzugt vollständige Projektpfade bei gleichen Dateinamen in mehreren Ordnern.
- LibreOffice verwendet kurze, pro Auftrag getrennte Profile im privaten Toolbox-Ordner.
  Damit erzeugt Calc auch unter tiefen Dokument-Arbeitspfaden eine Ausgabe. Die
  Windows-Pfadlänge wird vorab begrenzt; fehlgeschlagene Konvertierungen liefern
  begrenzte Diagnosen mit entfernten privaten Pfaden.
- Kurze Workspace-Sperren durch parallele Dateilisten-Abfragen beenden einen fortgesetzten
  Lauf nicht mehr sofort. Die Workspace-Prüfung wartet begrenzt auf diese Sperre;
  Identitätsfehler und dauerhaft belegte Workspaces bleiben Fehler. Werkzeugausführung
  und Veröffentlichung werden dadurch nicht wiederholt.
- Pflichtqualifikation mit gepinnten Abhängigkeiten ist für Windows/Linux in CI definiert.
  Fehlende Testvoraussetzungen dürfen nicht als grüne Skip-Fälle durchlaufen. Der
  Downloader besitzt eine feste Gesamtdauer; die Tests verändern keine vorhandene Laufzeit.
- Die Web-CSP erlaubt lokale Blob-URLs gezielt für Bild- und PDF-Vorschauen. Die
  Browserprüfung hatte zuvor blockierte PNGs nachgewiesen. Skript- und Objektregeln
  bleiben eng begrenzt; die HTTP-Prüfung kontrolliert die neuen Direktiven ausdrücklich.

Vertrag und HTTP-Ablauf: [DOCUMENT-ENGINE-PROTOCOL.md](DOCUMENT-ENGINE-PROTOCOL.md).
Quellen: `crates/runtime/src/document_actions.rs`, `crates/storage/src/document_actions.rs`,
`crates/toolbox/src/document_engine.rs`, `packages/ui/src/DocumentWorkbench.tsx`.

## Offene Umsetzung im vereinbarten Gesamtumfang

| Paket | Noch erforderlich | Abnahme |
|---|---|---|
| Fachansichten | Vollständiger Word-Struktureditor mit Runs/Tabellen/Feldern, Folien-/Objekteditor, Excel-Raster mit typisierten Zellen/Formatierung/Diagrammen, visuelle PDF-Bearbeitung sowie interaktive Bildfläche mit direktem Zuschnitt, Ebenen und exakter Vorschau. Die Bild-Schrittoberfläche besitzt bereits Parameterfelder, Undo/Redo und Entwürfe; Office-Textfelder bleiben Grundbedienung. | Unterstützte Operationen ohne Roh-JSON; Vorher/Nachher und gerenderte Ergebnisse; fachliche Indizes bleiben SHA-gebunden. |
| LaTeX-Projekte | Direkte Veröffentlichung bearbeiteter Projektquellen, größere begrenzte Asset-Sätze sowie ein breiter Bibliografie-/Paketkorpus. Manifest, Hauptdateiwechsel, gespeicherte Entwürfe und gebundene Quelltextnavigation sind implementiert. Aktuell gilt das 16-Dateien-Limit. | Mehrdateiprojekt mit Grafik/Bibliografie; Fehlerkorrektur; Neustart; fehlendes Paket und Grenzüberschreitungen. |
| Excel-Treue | Implementierten Referenz-/Objekterhalt an einem größeren Korpus und in Microsoft Excel absichern; Unterstützung für bisher abgelehnte Tabellen, komplexe Zeichnungen und Referenzarten ergänzen. Der qualifizierte Teilumfang und seine Grenzen stehen im Engine-Protokoll. | Richtige Referenzen/Ergebniswerte und definierter Objekterhalt in realem Excel/LibreOffice. |
| Word/PPTX/PDF/Bildqualität | Vorlagen-Rundlauf, Unicode-Schriften, Word-Mathematik/Felder, Folienüberlauf, Formular-Appearances, Transparenz und komplexe Originale qualifizieren. | Vergleich gerenderter Referenzen und echte Microsoft-Office-Abnahme. |
| OCR | Eigene lokale Laufzeit/Sprachen, begrenzte Seiten-Rasterung, Text-/Layout-/Konfidenzmodell, durchsuchbare PDFs und überprüfbare UI. | Deutsch/Englisch, schiefe Scans, Mehrspalten, große und fehlerhafte Eingaben; keine still behauptete Texterkennung. |
| Generative Bilder | Versionierter Jobvertrag, lokale Provider und expliziter Cloud-Opt-in, Masken/Referenzbilder, Modell-/Kosten-/Datentransferanzeige, Abbruch und Herkunft. | Reale lokale und gewählte Cloud-Ausführung; keine unbeabsichtigte Übertragung; Wiederherstellung ohne doppelte kostenpflichtige Jobs. |
| Workflows | Katalog/Agentenfilter, eigenes journalisiertes Dokumenteffekt-Dispatch, Vorschauübergabe, Freigaben, Budgets und Reconcile anschließen. | Workflow mit Lesen → Bericht/Präsentation/Bild; Prozessabbruch nach Veröffentlichung ohne Doppeldatei. |
| Weitere Ziele | Getrennte Team-Container, Offline-Freigaben, Geräteagent und Android anhand desselben versionierten Vertrags und expliziter Fähigkeiten anbinden. | Jedes Ziel separat mit Berechtigungs-, Abbruch-, Ergebnis- und Neustarttests. |
| Datenlebenszyklus | Neue Entwürfe/Vorbereitungen in Chat-Export/-Import und definierte Aufräum-/Aufbewahrungsregeln aufnehmen; PostgreSQL-Rennfälle in CI ausführen. | Export/Import erhält Quellenbindung, Historie und akzeptierte Aktionen; bestehende Archive bleiben lesbar. |
| Pakete und Interop | Aktuelle Windows-/Linux-Desktop- und Serverpakete, Docker-Images, Neuinstallation/Update und Gerätepakete ausliefern und qualifizieren. | Installierte Builds mit gebundenen Quellen-/Laufzeithashes, tatsächlichen Zielprogrammen und Modellaufrufen. |

Die nächsten Änderungen bauen auf dem gemeinsamen Dienst auf. Workflow-/Gerätewege dürfen
nicht durch bloßes Freischalten der Werkzeugnamen als unterstützt gelten: Ihre eigenen
Effektjournale und Berechtigungsgrenzen müssen zuerst angeschlossen und geprüft werden.

## Verifikation und Grenzen dieses Arbeitsstands

Die lokale Qualifikation nutzt ausschließlich synthetische Daten und private Testlaufzeiten.
Es wurden keine Nutzerdateien konvertiert, kein Cloud-Modell aufgerufen und kein App-Paket
installiert oder veröffentlicht. Ein im Test absichtlich gestörter Generate-Beleg wird nach
Daemon-Neustart exakt abgeglichen; es entsteht nur eine veröffentlichte Datei.

Die Windows-Engine-/Runtime-/Isolations- und HTTP-Ergebnisse stehen im zugehörigen
Prüfbeleg (development artifact omitted from public sources). Aktuell bestehen **134
Engine-Tests ohne Skip**, **32 UI-Tests**, **acht Runtime-Tests**, TypeScript und Webbuild.
Der aktuelle Daemon und Webbuild wurden gemeinsam im echten Browser geprüft: dekodierte
Quell-/Ergebnisbilder, Schrittfolge, Undo/Redo, Umordnen, Entwürfe nach Neuladen und
eine tatsächlich erzeugte 40×60-PNG-Datei. Hinzu kommen das gespeicherte LaTeX-Manifest,
Hauptdateiwechsel, echte UI-Kompilierung und Navigation zur richtigen Fehlerzeile einer
Begleitdatei. Der API-Teil prüft Mehrdateiprojekt/Grafik/Querverweis, Quellenkonflikte,
Korrektur, fehlendes Paket und Neustart. Frühere manuelle Browserbelege sind getrennt
als frühere Quellenstände aufgeführt.

Eine private LibreOffice-26.8.1-Laufzeit wurde aus dem SHA-gepinnten Archiv extrahiert.
Die direkte Referenzprüfung berechnet nach Zeileneinfügung und Blattumbenennung korrekte
Werte, einschließlich blattübergreifender Referenzen und benannter Bereiche. Die reale
Office-HTTP-Prüfung umfasst zusätzlich veraltete Formelcaches, Leerstrings, sichtbare
Rechenfehler und DOCX-/PPTX-/XLSX-Konvertierungen mit PDF-Text und PNG-Vorschauen. Sie
ersetzt keine Microsoft-Office-Abnahme. OCR-/GPU-Laufzeiten sind hier nicht vorhanden; die
Docker-Engine ist nicht gestartet. Linux-, PostgreSQL-, Microsoft-Office-Interop-, Cloud-
und installierte Paketabnahmen bleiben ausdrücklich offen. Für die aktuellen Build-,
Runtime- und HTTP-Prüfungen zählt ausschließlich ihr Status im Prüfbeleg; frühere
Prüfungen sind getrennt als frühere Quellenstände aufgeführt.

Der gesamte Checkout war bereits ungetrackt. Es gibt keinen neuen Commit, Push oder
Release. Vorhandene Arbeiten und historische Nachweise wurden nicht entfernt.
