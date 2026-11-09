import { create } from 'zustand'
import { persist } from 'zustand/middleware'

export type TaskTemplate = {
  id: string
  title: string
  description: string
  expectedOutput: string
  createdAt: number
  updatedAt: number
}

type TaskTemplateInput = {
  title?: string
  description: string
  expectedOutput?: string
}

type TaskTemplatesState = {
  templates: TaskTemplate[]
  addTemplate: (input: TaskTemplateInput) => string
  updateTemplate: (id: string, patch: Partial<Omit<TaskTemplate, 'id' | 'createdAt'>>) => void
  removeTemplate: (id: string) => void
}

function generateId(): string {
  return `tmpl-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function normalizeTemplate(template: Partial<TaskTemplate> & { id?: unknown }): TaskTemplate | null {
  if (typeof template.id !== 'string' || !template.id.trim()) return null

  const createdAt = typeof template.createdAt === 'number' ? template.createdAt : Date.now()
  const updatedAt = typeof template.updatedAt === 'number' ? template.updatedAt : createdAt

  return {
    id: template.id,
    title: typeof template.title === 'string' ? template.title : '',
    description: typeof template.description === 'string' ? template.description : '',
    expectedOutput: typeof template.expectedOutput === 'string' ? template.expectedOutput : '',
    createdAt,
    updatedAt,
  }
}

export const useTaskTemplatesStore = create<TaskTemplatesState>()(
  persist(
    (set) => ({
      templates: [],

      addTemplate: (input) => {
        const now = Date.now()
        const id = generateId()
        const template: TaskTemplate = {
          id,
          title: (input.title ?? '').trim(),
          description: input.description.trim(),
          expectedOutput: (input.expectedOutput ?? '').trim(),
          createdAt: now,
          updatedAt: now,
        }

        set((state) => ({
          templates: [template, ...state.templates],
        }))

        return id
      },

      updateTemplate: (id, patch) =>
        set((state) => ({
          templates: state.templates.map((template) =>
            template.id === id
              ? {
                  ...template,
                  ...patch,
                  title: patch.title !== undefined ? patch.title : template.title,
                  description: patch.description !== undefined ? patch.description : template.description,
                  expectedOutput: patch.expectedOutput !== undefined ? patch.expectedOutput : template.expectedOutput,
                  updatedAt: Date.now(),
                }
              : template,
          ),
        })),

      removeTemplate: (id) =>
        set((state) => ({
          templates: state.templates.filter((template) => template.id !== id),
        })),
    }),
    {
      name: 'open-cowork-task-templates',
      merge: (persistedState, currentState) => {
        const typedState = persistedState as Partial<TaskTemplatesState>
        const templates = Array.isArray(typedState.templates)
          ? typedState.templates
              .map((template) => normalizeTemplate(template))
              .filter((template): template is TaskTemplate => Boolean(template))
          : currentState.templates

        return {
          ...currentState,
          ...typedState,
          templates,
        }
      },
      partialize: (state) => ({
        templates: state.templates,
      }),
    },
  ),
)
