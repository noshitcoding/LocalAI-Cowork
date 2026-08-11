import { Building2, FolderPlus, Plus, Save, Trash2, UserPlus, Users, X } from 'lucide-react'
import { useCallback, useEffect, useMemo, useState, type FormEvent } from 'react'

import type { ProjectPrivacy, ProjectRecord, TeamMemberRecord, TeamRecord } from '../runtime/contracts'
import type { RemoteRuntimeClient } from '../runtime/runtimeClient'
import './RemoteManagement.css'

type Props = { client: RemoteRuntimeClient; currentUserId: string; compact?: boolean }

function messageOf(error: unknown): string { return error instanceof Error ? error.message : String(error) }

export default function RemoteOrganizationManager({ client, currentUserId, compact = false }: Props) {
  const [open, setOpen] = useState(false)
  const [teams, setTeams] = useState<TeamRecord[]>([])
  const [projects, setProjects] = useState<ProjectRecord[]>([])
  const [members, setMembers] = useState<TeamMemberRecord[]>([])
  const [teamId, setTeamId] = useState('')
  const [teamName, setTeamName] = useState('')
  const [memberId, setMemberId] = useState('')
  const [memberRole, setMemberRole] = useState<'admin' | 'member'>('member')
  const [projectName, setProjectName] = useState('')
  const [projectDescription, setProjectDescription] = useState('')
  const [projectPrivacy, setProjectPrivacy] = useState<ProjectPrivacy>('private_local')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [nextTeams, nextProjects] = await Promise.all([client.listTeams(), client.listProjects()])
      setTeams(nextTeams)
      setProjects(nextProjects)
      setTeamId((current) => nextTeams.some((team) => team.id === current) ? current : nextTeams[0]?.id ?? '')
      setError(null)
    } catch (cause) { setError(messageOf(cause)) }
  }, [client])
  const loadMembers = useCallback(async () => {
    if (!teamId) { setMembers([]); return }
    try { setMembers(await client.listTeamMembers(teamId)); setError(null) }
    catch (cause) { setMembers([]); setError(messageOf(cause)) }
  }, [client, teamId])
  useEffect(() => { if (open) void load() }, [load, open])
  useEffect(() => { if (open) void loadMembers() }, [loadMembers, open])

  const currentMembership = members.find((member) => member.user_id === currentUserId)
  const canManageTeam = currentMembership?.role === 'owner' || currentMembership?.role === 'admin'
  const selectedTeam = teams.find((team) => team.id === teamId)
  const canDeleteProject = useCallback((project: ProjectRecord) => (
    project.owner_user_id === currentUserId
    || (project.team_id === teamId && canManageTeam)
  ), [canManageTeam, currentUserId, teamId])
  const teamProjectAllowed = Boolean(teamId && canManageTeam)
  useEffect(() => {
    if (projectPrivacy === 'team_managed' && !teamProjectAllowed) setProjectPrivacy('private_local')
  }, [projectPrivacy, teamProjectAllowed])

  const createTeam = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true); setError(null)
    try {
      const team = await client.createTeam(teamName.trim())
      setTeamName(''); await load(); setTeamId(team.id)
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const saveMember = async (event: FormEvent) => {
    event.preventDefault(); if (!teamId) return
    setBusy(true); setError(null)
    try {
      await client.setTeamMember(teamId, memberId.trim(), memberRole)
      setMemberId(''); setMemberRole('member'); await loadMembers()
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const removeMember = async (member: TeamMemberRecord) => {
    setBusy(true); setError(null)
    try { await client.removeTeamMember(member.team_id, member.user_id); await loadMembers() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const createProject = async (event: FormEvent) => {
    event.preventDefault(); setBusy(true); setError(null)
    try {
      await client.createProject({
        name: projectName.trim(), description: projectDescription.trim(), privacy: projectPrivacy,
        team_id: projectPrivacy === 'team_managed' ? teamId : null,
        preferred_executor_target: null, policy: { tool_policy: 'autonomous' },
      })
      setProjectName(''); setProjectDescription(''); await load()
    } catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const removeProject = async (project: ProjectRecord) => {
    setBusy(true); setError(null)
    try { await client.deleteProject(project.id, project.revision); await load() }
    catch (cause) { setError(messageOf(cause)) } finally { setBusy(false) }
  }
  const visibleProjects = useMemo(() => projects.filter((project) => (
    !project.team_id || !teamId || project.team_id === teamId
  )), [projects, teamId])

  if (!open) return <button className={compact ? '' : 'ui-button ui-button--secondary ui-button--sm'} type="button" onClick={() => setOpen(true)}><Building2 size={14} /> Projects</button>
  return (
    <section className={`remote-management-panel remote-organization-manager${compact ? ' compact' : ''}`}>
      <header><div><Building2 size={16} /><strong>Teams and projects</strong></div><button type="button" aria-label="Close teams and projects" onClick={() => setOpen(false)}><X size={15} /></button></header>
      <p className="remote-management-hint">Private project files remain on your device. Team projects keep their versioned files on the server.</p>
      <div className="remote-organization-grid">
        <div>
          <strong><Users size={14} /> Teams</strong>
          <div className="remote-management-list">
            {teams.length === 0 ? <p>No teams yet.</p> : teams.map((team) => <article key={team.id} className={team.id === teamId ? 'selected' : ''}><button type="button" onClick={() => setTeamId(team.id)}><span><strong>{team.name}</strong><small>{team.owner_user_id === currentUserId ? 'Owner' : team.id}</small></span></button></article>)}
          </div>
          <form onSubmit={createTeam}><label>New team name<input value={teamName} onChange={(event) => setTeamName(event.target.value)} maxLength={200} required /></label><div className="remote-management-actions"><button type="submit" disabled={busy || !teamName.trim()}><Plus size={14} /> Create team</button></div></form>
        </div>
        <div>
          <strong><UserPlus size={14} /> {selectedTeam ? `${selectedTeam.name} members` : 'Members'}</strong>
          <div className="remote-management-list">
            {!teamId ? <p>Select or create a team.</p> : members.map((member) => <article key={member.user_id}><span><strong>{member.display_name}</strong><small>{member.email} · {member.role}</small></span>{canManageTeam && member.role !== 'owner' ? <div><button type="button" aria-label={`Remove ${member.display_name}`} disabled={busy} onClick={() => { void removeMember(member) }}><Trash2 size={14} /></button></div> : null}</article>)}
          </div>
          {canManageTeam ? <form onSubmit={saveMember}><label>User ID<input value={memberId} onChange={(event) => setMemberId(event.target.value)} pattern="[0-9a-fA-F-]{36}" required /></label><label>Role<select value={memberRole} onChange={(event) => setMemberRole(event.target.value as 'admin' | 'member')}><option value="member">Member</option><option value="admin">Administrator</option></select></label><div className="remote-management-actions"><button type="submit" disabled={busy || !memberId.trim()}><Save size={14} /> Add or update</button></div></form> : teamId ? <p className="remote-management-hint">Only team owners and administrators can change membership.</p> : null}
        </div>
      </div>
      <div className="remote-management-list remote-organization-projects">
        <strong><FolderPlus size={14} /> Projects</strong>
        {visibleProjects.length === 0 ? <p>No accessible projects.</p> : visibleProjects.map((project) => <article key={project.id}><span><strong>{project.name}</strong><small>{project.privacy === 'private_local' ? 'Private files on personal devices' : teams.find((team) => team.id === project.team_id)?.name ?? 'Team project'}</small></span>{canDeleteProject(project) ? <div><button type="button" aria-label={`Delete ${project.name}`} disabled={busy} onClick={() => { void removeProject(project) }}><Trash2 size={14} /></button></div> : null}</article>)}
      </div>
      <form onSubmit={createProject}>
        <label>Project name<input value={projectName} onChange={(event) => setProjectName(event.target.value)} maxLength={200} required /></label>
        <label>Storage<select value={projectPrivacy} onChange={(event) => setProjectPrivacy(event.target.value as ProjectPrivacy)}><option value="private_local">Private, files stay local</option>{teamProjectAllowed ? <option value="team_managed">Team-managed server files</option> : null}</select></label>
        <label>Description<textarea value={projectDescription} onChange={(event) => setProjectDescription(event.target.value)} rows={3} /></label>
        <div className="remote-management-actions"><button type="submit" disabled={busy || !projectName.trim()}><FolderPlus size={14} /> Create project</button></div>
      </form>
      {error ? <div className="remote-inline-error" role="alert">{error}</div> : null}
    </section>
  )
}
