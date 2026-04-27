import { useEffect, useState } from 'react'
import { useCoworkStore } from '../stores/coworkStore'

export default function SchedulerPanel() {
  const {
    scheduledTasks,
    scheduledRuns,
    loadScheduledTasks,
    loadScheduledRuns,
    upsertScheduledTask,
    toggleScheduledTask,
    runScheduledTaskNow,
    removeScheduledTask,
  } = useCoworkStore()
  const [name, setName] = useState('')
  const [scheduleExpr, setScheduleExpr] = useState('daily 09:00')
  const [prompt, setPrompt] = useState('')

  useEffect(() => {
    void loadScheduledTasks()
    void loadScheduledRuns(20)
  }, [loadScheduledRuns, loadScheduledTasks])

  const inputStyle: React.CSSProperties = {
    padding: '6px 10px',
    borderRadius: 'var(--radius-sm)',
    border: '1px solid var(--border-color)',
    background: 'var(--bg-secondary)',
    fontSize: 13,
    width: '100%',
  }

  const handleCreate = async () => {
    if (!name.trim() || !prompt.trim() || !scheduleExpr.trim()) return
    await upsertScheduledTask({
      id: crypto.randomUUID(),
      name: name.trim(),
      prompt: prompt.trim(),
      cronLike: scheduleExpr.trim(),
      active: true,
      lastRunAt: null,
      nextRunAt: null,
    })
    setName('')
    setPrompt('')
  }

  return (
    <div className="panel">
      <h2>⏰ Scheduler</h2>

      <div className="card" style={{ marginBottom: 12, display: 'flex', flexDirection: 'column', gap: 6 }}>
        <input placeholder="Name" value={name} onChange={(event) => setName(event.target.value)} style={inputStyle} />
        <input placeholder="Ausdruck, z.B. daily 09:00" value={scheduleExpr} onChange={(event) => setScheduleExpr(event.target.value)} style={inputStyle} />
        <textarea
          placeholder="Aufgabe"
          value={prompt}
          onChange={(event) => setPrompt(event.target.value)}
          rows={3}
          style={{ ...inputStyle, resize: 'vertical' }}
        />
        <button type="button" className="btn-sm" onClick={() => void handleCreate()}>
          Task anlegen
        </button>
      </div>

      {scheduledTasks.length === 0 ? (
        <p className="panel-empty">Keine geplanten Tasks vorhanden.</p>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          {scheduledTasks.map((task) => (
            <div key={task.id} className="card" style={{ display: 'flex', justifyContent: 'space-between', gap: 10 }}>
              <div style={{ flex: 1 }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <strong>{task.name}</strong>
                  <span style={{ fontSize: 11, padding: '1px 6px', borderRadius: 8, background: task.active ? 'var(--success)' : 'var(--danger)', color: '#fff' }}>
                    {task.active ? 'aktiv' : 'pausiert'}
                  </span>
                </div>
                <div style={{ fontSize: 12, color: 'var(--text-muted)' }}>{task.cronLike}</div>
                <div style={{ fontSize: 12, color: 'var(--text-secondary)', marginTop: 4 }}>{task.prompt}</div>
                <div style={{ fontSize: 11, color: 'var(--text-muted)', marginTop: 6 }}>
                  Letzter Lauf: {task.lastRunAt ? new Date(task.lastRunAt).toLocaleString('de-DE') : 'noch keiner'}
                  {' · '}
                  Naechster Lauf: {task.nextRunAt ? new Date(task.nextRunAt).toLocaleString('de-DE') : 'nicht geplant'}
                </div>
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                <button type="button" className="btn-sm" onClick={() => void toggleScheduledTask(task.id, !task.active)}>
                  {task.active ? 'Pause' : 'Aktivieren'}
                </button>
                <button type="button" className="btn-sm" onClick={() => void runScheduledTaskNow(task.id)}>
                  Jetzt ausfuehren
                </button>
                <button type="button" className="btn-sm" onClick={() => void removeScheduledTask(task.id)} style={{ color: 'var(--danger)' }}>
                  Entfernen
                </button>
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="card" style={{ marginTop: 12 }}>
        <strong style={{ display: 'block', marginBottom: 8 }}>Letzte Scheduler-Runs</strong>
        {scheduledRuns.length === 0 ? (
          <p className="panel-empty">Keine Ausfuehrungen protokolliert.</p>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            {scheduledRuns.slice(0, 8).map((run) => (
              <div key={run.id} style={{ fontSize: 12, borderBottom: '1px solid var(--border-color)', paddingBottom: 8 }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', gap: 8 }}>
                  <strong>{run.taskId}</strong>
                  <span>{run.status}</span>
                </div>
                <div style={{ color: 'var(--text-muted)', marginTop: 2 }}>
                  {new Date(run.startedAt).toLocaleString('de-DE')}
                </div>
                {(run.result || run.error) && (
                  <pre style={{ whiteSpace: 'pre-wrap', marginTop: 6, fontSize: 11 }}>{(run.error ?? run.result ?? '').slice(0, 280)}</pre>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}