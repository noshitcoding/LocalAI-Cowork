use super::*;
use crate::mcp_oauth::McpOAuthOwner;
impl TeamService{
    pub async fn team_mcp_oauth_profiles(&self,actor:&Principal,team:Uuid)->Result<Vec<McpOAuthProfile>>{self.store.mcp_oauth_profiles(&McpOAuthOwner::Team{actor,team,key:self.key_id},&format!("team:{team}"),self.keys.as_ref()).await}
    pub async fn discover_team_mcp_oauth(&self,actor:&Principal,team:Uuid,input:&McpOAuthDiscovery)->Result<McpOAuthMetadata>{self.store.discover_scoped_mcp_oauth(&McpOAuthOwner::Team{actor,team,key:self.key_id},&format!("team:{team}"),input,self.keys.as_ref()).await}
    pub async fn configure_team_mcp_oauth(&self,actor:&Principal,team:Uuid,input:&ConfigureMcpOAuth,redirect:&str)->Result<McpOAuthProfile>{self.store.configure_scoped_mcp_oauth(&McpOAuthOwner::Team{actor,team,key:self.key_id},&format!("team:{team}"),input,redirect,self.keys.as_ref()).await}
    pub async fn start_team_mcp_oauth(&self,actor:&Principal,team:Uuid,profile:&str,input:&McpOAuthRequest)->Result<McpOAuthFlow>{self.store.start_mcp_oauth(&McpOAuthOwner::Team{actor,team,key:self.key_id},profile,input,self.keys.as_ref()).await}
    pub async fn team_mcp_oauth_flow(&self,actor:&Principal,team:Uuid,flow:&str)->Result<McpOAuthFlow>{Ok(self.store.mcp_oauth_flow(&McpOAuthOwner::Team{actor,team,key:self.key_id},flow,self.keys.as_ref()).await?.view)}
    pub async fn complete_team_mcp_oauth(&self,actor:&Principal,team:Uuid,flow:&str)->Result<Value>{self.store.complete_mcp_oauth_code(&McpOAuthOwner::Team{actor,team,key:self.key_id},flow,self.keys.as_ref()).await}
    pub async fn team_mcp_tasks(&self,actor:&Principal,team:Uuid,run:Uuid)->Result<Vec<McpAsyncTask>>{self.store.scoped_mcp_tasks(&McpOAuthOwner::Team{actor,team,key:self.key_id},&run.to_string(),self.keys.as_ref()).await}
    pub async fn team_mcp_task_detail(&self,actor:&Principal,team:Uuid,run:Uuid,id:&str)->Result<McpTaskDetail>{self.store.scoped_mcp_task_detail(&McpOAuthOwner::Team{actor,team,key:self.key_id},&run.to_string(),id,self.keys.as_ref()).await}
    pub async fn cancel_team_mcp_task(&self,actor:&Principal,team:Uuid,run:Uuid,id:&str,input:&McpAsyncTaskRequest)->Result<Value>{self.store.request_scoped_mcp_task_cancel(&McpOAuthOwner::Team{actor,team,key:self.key_id},&run.to_string(),id,input,self.keys.as_ref()).await}
    pub async fn team_mcp_interactions(&self,actor:&Principal,team:Uuid,run:Uuid)->Result<Vec<McpTaskInteraction>>{self.store.mcp_task_interactions(&McpOAuthOwner::Team{actor,team,key:self.key_id},&run.to_string(),self.keys.as_ref()).await}
    pub async fn answer_team_mcp_interaction(&self,actor:&Principal,team:Uuid,run:Uuid,id:&str,input:&AnswerMcpTaskInteraction)->Result<Value>{self.store.answer_mcp_task_interaction(&McpOAuthOwner::Team{actor,team,key:self.key_id},&run.to_string(),id,input,self.keys.as_ref()).await}
}
