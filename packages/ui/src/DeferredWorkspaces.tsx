import {Component, lazy, Suspense, useMemo, useState, type ComponentProps, type ComponentType, type ReactNode} from "react";
import type {SettingsWorkspaceProps} from "./SettingsWorkspace";
import {useView} from "./view";

class WorkspaceErrorBoundary extends Component<{children: ReactNode; language: "de" | "en"; retry: () => void}, {failed: boolean}> {
  state = {failed: false};
  static getDerivedStateFromError() {return {failed: true};}
  render() {
    if (!this.state.failed) return this.props.children;
    const german = this.props.language === "de";
    return <div className="error deferred-workspace-error" role="alert"><p>{german
      ? "Die Ansicht konnte nicht geladen werden. Dein Chat bleibt geöffnet. Prüfe die Verbindung und versuche es erneut."
      : "This workspace could not load. Your chat remains open. Check the connection and try again."}</p>
      <button type="button" onClick={this.props.retry}>{german ? "Erneut versuchen" : "Try again"}</button></div>;
  }
}

function deferred<P extends object>(load: () => Promise<{default: ComponentType<P>}>) {
  return function DeferredWorkspace(props: P) {
    const language = useView(view => view.language);
    const [attempt, setAttempt] = useState(0);
    // A rejected React.lazy promise is cached; retry uses a fresh identity.
    const View = useMemo(() => lazy(load), [attempt]);
    return <WorkspaceErrorBoundary key={attempt} language={language} retry={() => setAttempt(value => value + 1)}>
      <Suspense fallback={<p className="muted" role="status">{language === "de" ? "Ansicht wird geladen …" : "Loading workspace …"}</p>}><View {...props}/></Suspense>
    </WorkspaceErrorBoundary>;
  };
}

// Visited views remain mounted, retaining drafts when navigating away and back.
export const SettingsWorkspace = deferred<SettingsWorkspaceProps>(() => import("./SettingsWorkspace").then(module => ({default: module.SettingsWorkspace})));
export const OfficeWorkbench = deferred<ComponentProps<typeof import("./OfficeWorkbench").OfficeWorkbench>>(() => import("./OfficeWorkbench").then(module => ({default: module.OfficeWorkbench})));
export const DeveloperWorkspace = deferred<ComponentProps<typeof import("./DeveloperWorkspace").DeveloperWorkspace>>(() => import("./DeveloperWorkspace").then(module => ({default: module.DeveloperWorkspace})));
export const TerminalDock = deferred<ComponentProps<typeof import("./TerminalDock").TerminalDock>>(() => import("./TerminalDock").then(module => ({default: module.TerminalDock})));
export const WorkflowLibrary = deferred<ComponentProps<typeof import("./WorkflowLibrary").WorkflowLibrary>>(() => import("./WorkflowLibrary").then(module => ({default: module.WorkflowLibrary})));
export const TeamPanel = deferred<ComponentProps<typeof import("./TeamPanel").TeamPanel>>(() => import("./TeamPanel").then(module => ({default: module.TeamPanel})));
