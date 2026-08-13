# Ollama Konfiguration

## Ziel

Die Modellanbindung ist auf lokale bzw. netzinterne Ollama-Instanzen ausgelegt.

Standardendpoint im Projekt:
- `http://localhost:11434`

## Konfigurationsquellen

Reihenfolge:
1. UI-Eingabe (persistiert lokal)
2. Environment-Variablen
3. Code-Defaults

Verwendete Variablen:
- `OLLAMA_BASE_URL`
- `OLLAMA_MODEL`
- `OLLAMA_TIMEOUT_MS`

## API-Endpunkte

Die App nutzt aktuell:
- `GET /api/tags` fuer Modellliste und Erreichbarkeit
- `GET /api/version` fuer Serverversion (optional)
- `POST /api/generate` fuer Plan-Generierung

## Beispiel: lokaler Start von Ollama

```powershell
ollama serve
ollama pull llama3.1:8b
```

## Fehlerbilder und Diagnose

## 1. Endpoint nicht erreichbar

Symptom:
- Health-Check liefert Fehler

Pruefung:

```powershell
curl http://localhost:11434/api/tags
```

Massnahmen:
- Ollama-Dienst starten
- Firewall/Port 11434 pruefen
- Endpoint in UI anpassen

## 2. Modell nicht vorhanden

Symptom:
- `/api/generate` liefert Fehler

Massnahme:

```powershell
ollama pull llama3.1:8b
```

## 3. Timeouts

Symptom:
- Requests brechen nach Timeout ab

Massnahmen:
- `timeoutMs` in UI erhoehen
- kleinere Modelle nutzen
- Host-Ressourcen prüfen

## Produktionshinweise

- Endpoint als Umgebungskonfiguration setzen, nicht hardcoden
- Modellliste fuer Deployment freigeben und dokumentieren
- Monitoring fuer Latenz, Fehlerraten und Timeouts einrichten
