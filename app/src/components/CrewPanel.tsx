import { useEffect, useMemo, useState } from 'react'
import { useCrewStore } from '../stores/crewStore'

export default function CrewPanel() {
  const {
    crews,
    agents,
    executionLogs,
    activeCrewId,
    createCrew,
    deleteCrew,
    setActiveCrew,
    addTask,
    runCrew,
    stopCrew,
    loadAgents,
    installDefaultAgents,
  } = useCrewStore()

  const [crewName, setCrewName] = useState('')
  const [taskDescription, setTaskDescription] = useState('')
  const [taskExpectedOutput, setTaskExpectedOutput] = useState('')
  const [taskAgentId, setTaskAgentId] = useState('')

  useEffect(() => {
    loadAgents()
    installDefaultAgents()
  }, [installDefaultAgents, loadAgents])

  useEffect(() => {
    if (!taskAgentId && agents.length > 0) {
      setTaskAgentId(agents[0].id)
    }
  }, [agents, taskAgentId])

  const activeCrew = useMemo(
    () => crews.find((crew) => crew.id === activeCrewId) ?? crews[0],
    [activeCrewId, crews],
  )

  const inputStyle: React.CSSProperties = {
    padding: '6px 10px',
    borderRadius: 'var(--radius-sm)',
    border: '1px solid var(--border-color)',
    background: 'var(--bg-secondary)',
    fontSize: 13,
    width: '100%',
  }

  const handleCreateCrew = () => {
    if (!crewName.trim()) return
    const id = crypto.randomUUID()
    createCrew(id, crewName.trim(), [])
    setActiveCrew(id)
    setCrewName('')
  }

  const handleAddTask = () => {
    if (!activeCrew || !taskDescription.trim() || !taskAgentId) return
    addTask(activeCrew.id, {
      id: crypto.randomUUID(),
      description: taskDescription.trim(),
      expectedOutput: taskExpectedOutput.trim(),
      agentId: taskAgentId,
      context: [],
      dependencies: [],
      asyncExecution: false,
      status: 'pending',
      output: null,
    })
    setTaskDescription('')
    setTaskExpectedOutput('')
  }

  return (
    <div className="panel">
      <h2>🚀 Crew AI</h2>

      <div className="card" style={{ marginBottom: 12, display: 'flex', gap: 8 }}>
        <input placeholder="Neue Crew" value={crewName} onChange={(event) => setCrewName(event.target.value)} style={inputStyle} />
        <button type="button" className="btn-sm" onClick={handleCreateCrew}>Crew anlegen</button>
      </div>

      {crews.length === 0 ? (
        <p className="panel-empty">Noch keine Crew vorhanden.</p>
      ) : (
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1.2fr', gap: 12 }}>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
            {crews.map((crew) => (
              <div
                key={crew.id}
                className="card"
                style={{ border: activeCrew?.id === crew.id ? '1px solid var(--accent)' : '1px solid var(--border-color)' }}
              >
                <div style={{ display: 'flex', justifyContent: 'space-between', gap: 8 }}>
                  <button type="button" className="btn-sm" onClick={() => setActiveCrew(crew.id)}>
                    {crew.name}
                  </button>
                  <button type="button" className="btn-sm" onClick={() => deleteCrew(crew.id)} style={{ color: 'var(--danger)' }}>
                    ✕
                  </button>
                </div>
                <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 6 }}>
                  {crew.process} · {crew.status} · {crew.tasks.length} Tasks
                </div>
                <div style={{ display: 'flex', gap: 6, marginTop: 8 }}>
                  <button type="button" className="btn-sm" onClick={() => void runCrew(crew.id)} disabled={crew.tasks.length === 0 || crew.status === 'running'}>
                    Run
                  </button>
                  <button type="button" className="btn-sm" onClick={() => void stopCrew(crew.id)} disabled={crew.status !== 'running'}>
                    Stop
                  </button>
                </div>
              </div>
            ))}
          </div>

          <div className="card">
            {activeCrew ? (
              <>
                <div style={{ marginBottom: 10 }}>
                  <strong>{activeCrew.name}</strong>
                  <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 4 }}>
                    Status: {activeCrew.status}
                  </div>
                </div>

                <div style={{ display: 'flex', flexDirection: 'column', gap: 6, marginBottom: 12 }}>
                  <textarea
                    placeholder="Task-Beschreibung"
                    value={taskDescription}
                    onChange={(event) => setTaskDescription(event.target.value)}
                    rows={3}
                    style={{ ...inputStyle, resize: 'vertical' }}
                  />
                  <input placeholder="Erwartetes Ergebnis" value={taskExpectedOutput} onChange={(event) => setTaskExpectedOutput(event.target.value)} style={inputStyle} />
                  <select value={taskAgentId} onChange={(event) => setTaskAgentId(event.target.value)} style={inputStyle}>
                    {agents.map((agent) => <option key={agent.id} value={agent.id}>{agent.name} ({agent.role})</option>)}
                  </select>
                  <button type="button" className="btn-sm" onClick={handleAddTask}>Task hinzufuegen</button>
                </div>

                <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                  {activeCrew.tasks.length === 0 ? (
                    <p className="panel-empty">Keine Tasks definiert.</p>
                  ) : (
                    activeCrew.tasks.map((task) => (
                      <div key={task.id} style={{ borderBottom: '1px solid var(--border-color)', paddingBottom: 8 }}>
                        <strong>{task.description}</strong>
                        <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 2 }}>
                          {task.status} · Agent: {agents.find((agent) => agent.id === task.agentId)?.name ?? task.agentId}
                        </div>
                        {task.output && <pre style={{ whiteSpace: 'pre-wrap', marginTop: 6, fontSize: 11 }}>{task.output.slice(0, 320)}</pre>}
                      </div>
                    ))
                  )}
                </div>
              </>
            ) : (
              <p className="panel-empty">Keine Crew ausgewaehlt.</p>
            )}
          </div>
        </div>
      )}

      <div className="card" style={{ marginTop: 12 }}>
        <strong style={{ display: 'block', marginBottom: 8 }}>Execution Logs</strong>
        {executionLogs.length === 0 ? (
          <p className="panel-empty">Keine Crew-Logs vorhanden.</p>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8, maxHeight: 260, overflowY: 'auto' }}>
            {executionLogs.slice(0, 20).map((log) => (
              <div key={log.id} style={{ fontSize: 12, borderBottom: '1px solid var(--border-color)', paddingBottom: 8 }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', gap: 8 }}>
                  <strong>{log.action}</strong>
                  <span>{new Date(log.timestamp).toLocaleTimeString('de-DE')}</span>
                </div>
                <div style={{ color: 'var(--text-muted)', marginTop: 2 }}>{log.agentId} · {log.taskId}</div>
                <pre style={{ whiteSpace: 'pre-wrap', marginTop: 6, fontSize: 11 }}>{log.result}</pre>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}