import { describe, expect, it } from 'vitest'
import { getModelGuidance, getModelOptionLabel } from './modelGuidance'

describe('modelGuidance', () => {
  it.each([
    ['nomic-embed-text', 'Search and retrieval model'],
    ['llava:13b', 'Image and text model'],
    ['qwen2.5-coder:7b', 'Programming model'],
    ['deepseek-r1:32b', 'Reasoning-focused model'],
    ['llama3.1:8b', 'Fast everyday model'],
    ['llama3.3:70b', 'High-capacity model'],
    ['custom-model', 'General-purpose model'],
  ])('classifies %s', (model, title) => {
    expect(getModelGuidance(model).title).toBe(title)
  })

  it('explains common terms in the technical model name', () => {
    const guidance = getModelGuidance('qwen2.5-vl:7b-instruct-q4')
    const explanations = guidance.nameExplanations.map((explanation) => explanation.text).join(' ')

    expect(guidance.nameExplanations).toContainEqual(expect.objectContaining({ values: { size: '7' } }))
    expect(guidance.nameExplanations).toContainEqual(expect.objectContaining({ values: { quantization: 'Q4' } }))
    expect(explanations).toContain('{{size}}B')
    expect(explanations).toContain('VL')
    expect(explanations).toContain('{{quantization}}')
    expect(explanations).toContain('Instruct')
  })

  it('adds the recommended task type without changing the model identifier', () => {
    expect(getModelOptionLabel('codellama:13b')).toBe('codellama:13b — Programming model')
  })
})
