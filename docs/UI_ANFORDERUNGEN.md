# UI-Anforderungsliste fuer Open_Cowork

## 1. Zweck und Scope

Dieses Dokument beschreibt ausschliesslich die Anforderungen an die Benutzeroberflaeche von Open_Cowork als Windows-Desktop-App. Backend-, Sicherheits-, Persistenz- und Integrationsanforderungen sind nur dann enthalten, wenn sie direkt in der UI sichtbar oder bedienbar sein muessen.

Die Anforderungen sind auf die bestehende Produktstruktur ausgerichtet:

- Welcome Screen
- Cowork-Hauptansicht
- linke Sidebar mit Threads und Navigation
- rechte Kontext- und Statuspanels
- Features-Ansicht
- Settings-Ansicht

## 2. UI-Ziele

- Die UI muss agentisches Arbeiten in einem zusammenhaengenden Arbeitsfluss unterstuetzen.
- Die UI muss Planen, Freigeben, Ausfuehren und Nachvollziehen sichtbar voneinander trennen.
- Die UI muss fuer laengere Desktop-Sessions schnell bedienbar, keyboard-first und visuell stabil sein.
- Die UI muss auch bei mehreren Threads, Tasks und Panels uebersichtlich bleiben.
- Die UI muss technische Komplexitaet reduzieren, ohne Kontrollverlust zu erzeugen.

## 3. Gestaltungsprinzipien

### UI-001 Klarer Arbeitskontext

- Die aktive Aufgabe, der aktive Thread und der aktuelle Arbeitsstatus muessen jederzeit eindeutig erkennbar sein.
- Die Hauptansicht darf nie unklar lassen, ob sich der Nutzer im Chat-, Plan-, Task-, Feature- oder Einstellungsbereich befindet.

### UI-002 Progressive Offenlegung

- Komplexe Steuerungen duerfen nicht alle gleichzeitig sichtbar sein.
- Erweiterte Optionen muessen kontextbezogen, aufklappbar oder in Nebenpanels organisiert sein.

### UI-003 Kontrollierbarkeit

- Jede risikobehaftete Aktion muss sichtbar pruefbar, bestaetigbar, pausierbar oder abbrechbar sein.
- Die UI muss den Unterschied zwischen Vorschlag, Freigabe und Ausfuehrung eindeutig markieren.

### UI-004 Desktop-First Bedienung

- Die Oberflaeche muss fuer Maus, Tastatur, grosse Fenster, schmale Fenster und Multi-Panel-Nutzung optimiert sein.
- Wiederkehrende Kernaktionen muessen ueber Shortcuts oder direkte Schnellaktionen erreichbar sein.

### UI-005 Konsistenz

- Gleichartige Status, Buttons, Panels, Dialoge und Listen muessen ueber alle Ansichten hinweg denselben Interaktionsregeln folgen.
- Terminologie und visuelle Semantik muessen in allen Ansichten identisch sein.

## 4. Informationsarchitektur

### UI-010 Hauptnavigation

- Die Hauptnavigation muss mindestens die Bereiche Welcome, Cowork, Features und Einstellungen abbilden.
- Der aktuell aktive Bereich muss visuell hervorgehoben sein.
- Navigation darf keine Datenverluste verursachen; ungespeicherte Eingaben oder laufende Aktionen muessen geschuetzt werden.

### UI-011 Linke Sidebar

- Die linke Sidebar muss Thread-Liste, Schnellnavigation und den Einstieg in neue Arbeitskontexte enthalten.
- Threads muessen scanbar sein und mindestens Titel, Aktualitaet und Aktivstatus zeigen.
- Lange Listen muessen filterbar oder suchbar sein.

### UI-012 Hauptarbeitsflaeche

- Die zentrale Flaeche muss den primaeren Arbeitskontext tragen.
- In der Cowork-Ansicht muss der Nutzer den Nachrichtenverlauf, Eingabe, Planungsstatus und relevante Folgeaktionen sehen koennen.
- Die Hauptarbeitsflaeche darf nicht durch sekundaire Meta-Informationen ueberladen werden.

### UI-013 Rechte Kontextzone

- Kontextinformationen wie Plan, Task-Status, Schritte, Artefakte, Insights oder Prozesse muessen in einer klar getrennten rechten Zone oder in eindeutigen Sekundaerpanels erscheinen.
- Die rechte Zone muss den Hauptfluss ergaenzen, darf ihn aber nicht verdecken oder dominieren.

## 5. Anforderungen pro Ansicht

### 5.1 Welcome Screen

#### UI-020 Einstieg und Orientierung

- Der Welcome Screen muss fuer Erstnutzung und Rueckkehrer verstaendlich sein.
- Er muss erklaeren, was Open_Cowork kann, wie ein erster Task gestartet wird und welche naechsten Schritte sinnvoll sind.
- Er muss mindestens eine primaere Call-to-Action fuer den ersten Task enthalten.

#### UI-021 Guided Onboarding

- Es muss einen gefuehrten Einstieg fuer neue Nutzer geben.
- Der Guided Flow muss mindestens Modellstatus, Einstieg in den ersten Prompt, zentrale Bedienlogik und Freigabekonzept erklaeren.
- Der Onboarding-Status muss schliessbar und spaeter erneut aufrufbar sein.

### 5.2 Cowork-Hauptansicht

#### UI-030 Prompteingabe

- Das Eingabefeld muss fuer kurze und lange Arbeitsauftraege geeignet sein.
- Es muss erkennbare Sende-, Abbruch- und gegebenenfalls Folgeaktionsmoeglichkeiten geben.
- Die Eingabe muss Tastatursteuerung, Mehrzeiligkeit und klare Fokusdarstellung unterstuetzen.

#### UI-031 Nachrichtenverlauf

- Nutzer- und System-/Agentenbeitraege muessen visuell klar unterscheidbar sein.
- Laengere Antworten muessen gut lesbar strukturiert sein, einschliesslich Listen, Code, Statushinweisen und Zwischenergebnissen.
- Der Verlauf muss bei langen Sessions performant und scrollbar stabil bleiben.

#### UI-032 Thinking-, Status- und Streaming-Darstellung

- Die UI muss laufende Verarbeitung eindeutig anzeigen.
- Zwischenzustaende wie wartet, plant, fuehrt aus, braucht Freigabe, abgeschlossen oder fehlgeschlagen muessen sichtbar sein.
- Streaming-Inhalte duerfen kein Layout-Flackern oder Fokusverlust erzeugen.

#### UI-033 Folgeaktionen

- Nach einer Antwort muessen kontextbezogene Folgeaktionen sichtbar angeboten werden, zum Beispiel weiterfragen, Plan anzeigen, Task starten, Ergebnis uebernehmen oder verwerfen.
- Folgeaktionen muessen nah am betreffenden Inhalt erscheinen.

### 5.3 Plan-, Approval- und Task-UI

#### UI-040 Planansicht

- Vor der Ausfuehrung risikobehafteter Aufgaben muss ein Plan sichtbar dargestellt werden.
- Der Plan muss aus einzelnen, lesbaren Schritten bestehen.
- Jeder Schritt muss mindestens Zweck, erwartete Aktion und aktuellen Status zeigen.

#### UI-041 Approval-Interaktion

- Freigaben muessen als eigener, nicht uebersehbarer UI-Zustand erscheinen.
- Der Nutzer muss mindestens freigeben, ablehnen, anpassen oder abbrechen koennen.
- Die UI muss klar machen, welche Aktion genau freigegeben wird.

#### UI-042 Task-Lifecycle

- Tasks muessen die Zustaende erstellt, geplant, wartet auf Freigabe, laeuft, abgeschlossen, fehlgeschlagen und abgebrochen visuell unterscheiden.
- Laufende Tasks muessen pausiert oder gestoppt werden koennen, sofern der technische Ablauf dies unterstuetzt.
- Abgeschlossene Tasks muessen Ergebnis, Verlauf und relevante Artefakte anzeigen.

#### UI-043 Schritt- und Artefakttransparenz

- Ein Task muss seine Einzelschritte nachvollziehbar anzeigen.
- Zwischenergebnisse, Dateien, Logs, Tool-Ergebnisse oder Diffs muessen kontextbezogen einsehbar sein.
- Fehler muessen auf Schritt- oder Task-Ebene klar zugeordnet werden.

### 5.4 Features-Ansicht

#### UI-050 Feature-Katalog

- Die Features-Ansicht muss installierte und verfuegbare Cowork-Funktionen uebersichtlich darstellen.
- Features, Plugins, Skills oder persoenliche Erweiterungen muessen als klar unterscheidbare Einheiten erscheinen.
- Jede Einheit muss mindestens Name, Zweck, Status und moegliche Aktion anzeigen.

#### UI-051 Aktivierung und Konfiguration

- Feature-Aktivierung, Deaktivierung, Installation und Bearbeitung muessen eindeutig bedienbar sein.
- Der Nutzer muss erkennen koennen, welche Auswirkung eine Aenderung auf das Verhalten in der Cowork-Ansicht hat.

### 5.5 Settings-Ansicht

#### UI-060 Einstellungsarchitektur

- Einstellungen muessen thematisch gruppiert und ueber eine stabile Sekundaernavigation erreichbar sein.
- Die Kategorien muessen fuer Nutzer ohne technisches Detailwissen verstaendlich benannt sein.
- Lange Einstellungsseiten muessen visuell sauber gegliedert sein.

#### UI-061 Konfigurationsklarheit

- Kritische technische Einstellungen, zum Beispiel Modell, Basis-URL, MCP-Server oder Speicherverhalten, muessen erklaerende Hilfetexte haben.
- Gueltige, ungueltige und unvollstaendige Konfigurationen muessen sofort sichtbar sein.

#### UI-062 Rueckmeldung und Tests in Einstellungen

- Dort, wo Verbindungen, Modelle oder Integrationen konfiguriert werden, muss die UI einen direkten Test oder Health-Check anbieten.
- Testergebnisse muessen im selben Kontext als Erfolg, Warnung oder Fehler erscheinen.

## 6. Querschnittliche UI-Anforderungen

### UI-070 Globale Suche

- Es muss eine globale Suche ueber Threads, Tasks, Sessions, Artefakte und relevante Inhalte geben.
- Suchergebnisse muessen gruppiert, scanbar und direkt oeffnbar sein.
- Die Suche muss ueber Tastatur schnell aufrufbar sein.

### UI-071 Command Palette

- Es muss eine Command Palette fuer Navigation und Schnellaktionen geben.
- Sie muss mindestens Navigation, Thread-Wechsel, Task-Aktionen und zentrale Einstellungen abdecken.

### UI-072 Shortcuts

- Kernfunktionen muessen per Shortcut bedienbar sein.
- Shortcuts muessen sichtbar dokumentiert und im UI auffindbar sein.
- Konflikte mit nativen Windows-Konventionen sind zu vermeiden.

### UI-073 Benachrichtigungen

- Die UI muss lokale Benachrichtigungen fuer relevante Ereignisse unterstuetzen, zum Beispiel Plan bereit, Freigabe noetig, Task abgeschlossen oder Fehler.
- Benachrichtigungen muessen den Nutzer in den passenden Kontext zurueckfuehren koennen.

### UI-074 Lade-, Leer- und Fehlerzustaende

- Jede zentrale Ansicht muss definierte Lade-, Leer- und Fehlerzustaende haben.
- Leere Ansichten muessen handlungsorientiert formuliert sein.
- Fehlerzustaende muessen sowohl verstaendlich als auch technisch ausreichend konkret sein.

### UI-075 Statuskonsistenz

- Statusfarben, Icons, Labels und Badge-Semantik muessen app-weit konsistent sein.
- Erfolgs-, Warn-, Fehler- und Approval-Zustaende duerfen nicht mehrfach unterschiedlich codiert werden.

## 7. Desktop- und Fensterverhalten

### UI-080 Responsives Desktop-Layout

- Die App muss auf typischen Desktop-Breiten stabil funktionieren.
- Bei kleineren Fensterbreiten muessen Seitenleisten und Panels ein- oder ausblendbar sein.
- Das Layout darf bei Resize nicht unlesbar oder unbedienbar werden.

### UI-081 Fensterzustand und Persistenz

- Fensterposition, Fenstergroesse und relevante Layout-Zustaende sollen zwischen Sitzungen erhalten bleiben.
- Die UI muss nach Neustart in einen fuer den Nutzer erkennbaren und konsistenten Zustand zurueckkehren.

### UI-082 Fokusmanagement

- Fokus darf bei Navigation, Dialogen, Streaming, Panelwechseln oder Statusaenderungen nicht verloren gehen.
- Nach modalen Interaktionen muss der Fokus sinnvoll zurueckgesetzt werden.

## 8. Accessibility

### UI-090 Tastaturbedienung

- Alle Kernfunktionen muessen ohne Maus erreichbar sein.
- Fokusreihenfolge und Fokusindikatoren muessen nachvollziehbar und sichtbar sein.

### UI-091 Screenreader- und Semantik-Anforderungen

- Interaktive Elemente muessen semantisch korrekt ausgezeichnet sein.
- Statuswechsel, Fehlermeldungen und Freigabeaufforderungen muessen assistiven Technologien mitgeteilt werden.

### UI-092 Kontrast und Lesbarkeit

- Text, Icons, Rahmen und Statusindikatoren muessen ausreichende Kontraste aufweisen.
- Die UI muss auch bei vergroesserter Schrift und Kompaktmodus lesbar und bedienbar bleiben.

### UI-093 Zoom und Skalierung

- Die UI muss mindestens in den Bereichen 100 bis 200 Prozent Windows-Skalierung stabil funktionieren.
- Inhalt darf nicht abgeschnitten oder unzugaenglich werden.

## 9. Visuelle und textliche Anforderungen

### UI-100 Design System

- Die App muss auf einem konsistenten Set von Komponenten, Abstaenden, Typografie- und Farbregeln beruhen.
- Primaere, sekundaere und destruktive Aktionen muessen visuell klar unterscheidbar sein.

### UI-101 Dichte und Ruhe

- Informationsdichte muss hoch genug fuer Power-User sein, darf aber den Lesefluss nicht stoeren.
- Bereiche mit hoher Aktivitaet muessen visuell beruhigt und sauber gruppiert sein.

### UI-102 Sprache und Mikrotexte

- Beschriftungen, Statusmeldungen und Hilfetexte muessen kurz, eindeutig und handlungsorientiert formuliert sein.
- Technische Begriffe duerfen nur dort verwendet werden, wo der Nutzer sie fuer die Entscheidung braucht.

## 10. Abnahmekriterien fuer die UI

- Ein neuer Nutzer kann vom Welcome Screen aus ohne externe Hilfe einen ersten Task starten.
- Ein Nutzer erkennt in der Cowork-Ansicht jederzeit den aktiven Thread, den aktuellen Status und die naechste sinnvolle Aktion.
- Ein risikobehafteter Task kann vor der Ausfuehrung gelesen, bewertet und explizit freigegeben oder abgelehnt werden.
- Ein laufender oder abgeschlossener Task ist ueber Status, Schritte und Ergebnisse nachvollziehbar.
- Einstellungen fuer Modell, Features und Integrationen sind ohne Trial-and-Error konfigurierbar.
- Die Kernflows sind vollstaendig per Tastatur nutzbar.
- Die App bleibt bei langen Verlaeufen, mehreren Threads und parallelen Statusaenderungen visuell stabil.

## 11. Priorisierung fuer Open_Cowork

### Muss fuer den naechsten UI-Ausbau

- Welcome Screen mit Guided Onboarding
- klare Plan- und Approval-UI in der Cowork-Ansicht
- globale Suche ueber Threads, Tasks und Artefakte
- vollstaendige Command Palette und Shortcut-Abdeckung
- konsistente Task- und Schrittstatusdarstellung
- definierte Leer-, Lade- und Fehlerzustaende in allen Kernansichten
- Accessibility-Grundabdeckung fuer Tastatur, Fokus und Kontrast

### Soll nachziehen

- konfigurierbare Panel-Layouts
- persoenliche Ansichtspraefenzen je Nutzerprofil
- kontextbezogene Produktivitaetsvorschlaege und Folgeaktionen
- staerkere Visualisierung von Artefakten, Diffs und Prozessdaten

## 12. Bezug zu bestehender Dokumentation

Dieses Dokument konkretisiert den UI-Teil aus folgenden bestehenden Unterlagen:

- WINDOWS_DESKTOP_APP_ANFORDERUNGEN.md
- TRACEABILITY_MATRIX.md
- FEHLENDE_FEATURES_IM_CODE.md
- docs/ARCHITECTURE.md

Es dient als fokussierte Arbeitsgrundlage fuer UI-, UX- und Frontend-Umsetzung innerhalb des aktuellen Open_Cowork-Repositories.