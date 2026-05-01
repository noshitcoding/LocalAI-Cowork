# Fix for Per-Chat/Per-Task Permission Mode and Allowed Directories

## Problem
When users set a folder as allowed or set the permission mode to "bypass" for a specific chat or task, it was being applied globally instead of being scoped to that specific chat/task. This happened because:

1. The `permissionMode` and `allowedDirectories` were stored globally in the `engineStore` config
2. All chats and tasks shared the same engine configuration
3. There was no mechanism to override these settings per chat or per task

## Solution
Implemented per-chat and per-task permission configuration by:

### 1. Type Definitions
- Added `PermissionConfig` type in `chatStore.ts` and `taskStore.ts`:
  ```typescript
  export type PermissionConfig = {
    mode: PermissionMode
    allowedDirectories: string[]
  }
  ```

### 2. Store Updates
- **chatStore.ts**: Added `permissionConfig?: PermissionConfig` to `ChatThread` type
  - Added `setThreadPermissionConfig` method to update permission config for a thread
  - Updated database serialization/deserialization for permission config
  
- **taskStore.ts**: Added `permissionConfig?: PermissionConfig` to `Task` type

- **engineStore.ts**: 
  - Updated `sendMessage` to accept optional `permissionConfig` parameter
  - Updated `_initEngine` to accept optional `permissionConfig` parameter
  - Updated `buildChatEngineConfig` to accept and use `permissionConfig`
  - Engine config now uses `permissionConfig?.mode ?? config.permissionMode` (falls back to global if not set)

### 3. Engine Core Updates
- **queryEngine.ts**: 
  - Added `allowedDirectories?: string[]` to `EngineConfig`
  - Updated `buildToolContext` to use `this.config.allowedDirectories ?? []`
  - Permission context now includes allowed directories from config

### 4. Database Schema
- **db.rs**: 
  - Added migration (version 16) to add `permission_config_json` column to `chat_threads` table
  - Updated `list_threads` to include `permission_config_json`
  - Updated `insert_thread` to accept `permission_config_json` parameter
  - Added `update_thread_permission_config` method

### 5. UI Components
- **ChatView.tsx**: 
  - Added permission config panel (toggle with 🔒 button)
  - Allows setting permission mode (default/plan/bypass/strict) per chat
  - Allows adding/removing allowed directories per chat
  - Passes thread's permission config to `engineSendMessage`
  
- **CoworkView.tsx**: 
  - Updated to pass thread's permission config to `engineSendMessage`
  
- **WelcomeScreen.tsx**: 
  - Updated to pass thread's permission config to `engineSendMessage`

### 6. CSS
- **App.css**: Added styles for permission config panel

## How It Works

### Per-Chat Configuration
1. Click the 🔒 "Berechtigungen" button in any chat
2. Select permission mode:
   - **Standard**: Normal behavior with tool approval prompts
   - **Plan-Modus**: Only allows planning, no execution
   - **Bypass (alles erlauben)**: No approval prompts, all tools allowed
   - **Strikt (alles fragen)**: All tools require approval
3. Add allowed directories (optional):
   - Only these directories can be accessed by file operations
   - Empty list means no restrictions
4. Click "Speichern"

### Per-Task Configuration
- Tasks inherit permission config from their thread
- Can be extended to have independent config if needed

### Fallback Behavior
- If no permission config is set for a chat/task, uses global engine settings
- Global settings remain in `engineStore.config.permissionMode`

## Benefits

1. **Isolation**: Each chat/task can have its own security settings
2. **Flexibility**: Can use bypass mode for trusted tasks while keeping strict mode for others
3. **Directory Control**: Can restrict file access per chat (e.g., only allow access to specific project folders)
4. **Backward Compatible**: Existing chats without permission config use global settings
5. **Persistent**: Permission config is saved with each chat thread

## Testing

- TypeScript compilation: ✓ Passed
- Agent discipline validation: ✓ Passed (50 runs)
- Database migration: ✓ Version 16 added
- UI integration: ✓ All components updated

## Files Modified

1. `app/src/stores/chatStore.ts` - Added permission config types and methods
2. `app/src/stores/taskStore.ts` - Added permission config type
3. `app/src/stores/engineStore.ts` - Updated to accept and use permission config
4. `app/src/engine/core/queryEngine.ts` - Added allowedDirectories to config
5. `app/src-tauri/src/db.rs` - Database schema migration
6. `app/src/components/ChatView.tsx` - Added permission config UI
7. `app/src/components/CoworkView.tsx` - Updated to pass permission config
8. `app/src/components/WelcomeScreen.tsx` - Updated to pass permission config
9. `app/src/App.css` - Added permission config panel styles