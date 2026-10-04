import { useEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { Settings, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'

import { getCastBoardPluginConfig, updateCastBoardPluginConfig } from '../../lib/api'
import type {
  CastBoardPlugin,
  CastBoardPluginConfigField,
  CastBoardPluginConfigOverrides,
  CastBoardPluginConfigValue,
} from '../../lib/types'
import { Button } from '../ui/button'
import { Input } from '../ui/input'
import { Select } from '../ui/select'
import { Slider } from '../ui/slider'
import { Switch } from '../ui/switch'

interface PluginConfigDialogProps {
  plugin: CastBoardPlugin
  language: string
  onClose: () => void
}

function localizedText(values: Record<string, string> | undefined, language: string, fallback: string) {
  if (!values) return fallback
  const normalized = language.toLowerCase()
  const exact = Object.entries(values).find(([key]) => key.toLowerCase() === normalized)?.[1]
  if (exact) return exact
  const base = normalized.split('-')[0]
  return Object.entries(values).find(([key]) => key.toLowerCase() === base)?.[1]
    ?? values.en
    ?? Object.values(values)[0]
    ?? fallback
}

function defaultValues(plugin: CastBoardPlugin): CastBoardPluginConfigOverrides {
  return Object.fromEntries(
    Object.entries(plugin.configSchema?.properties ?? {}).map(([name, field]) => [name, field.default]),
  )
}

function validValue(value: CastBoardPluginConfigValue, field: CastBoardPluginConfigField) {
  if (field.type === 'boolean') return typeof value === 'boolean'
  if (field.type === 'string') return typeof value === 'string' && (!field.enum || field.enum.includes(value))
  return typeof value === 'number'
    && Number.isFinite(value)
    && (field.type !== 'integer' || Number.isInteger(value))
    && (field.minimum === undefined || value >= field.minimum)
    && (field.maximum === undefined || value <= field.maximum)
}

export function PluginConfigDialog({ plugin, language, onClose }: PluginConfigDialogProps) {
  const { t } = useTranslation()
  const defaults = useMemo(() => defaultValues(plugin), [plugin])
  const [values, setValues] = useState<CastBoardPluginConfigOverrides | null>(null)
  const [saving, setSaving] = useState(false)
  const schema = plugin.configSchema

  useEffect(() => {
    let disposed = false
    void getCastBoardPluginConfig(plugin.id).then((overrides) => {
      if (!disposed) setValues({ ...defaults, ...overrides })
    }).catch(() => {
      if (!disposed) toast.error(t('castboard.plugins.errors.config'))
    })
    return () => { disposed = true }
  }, [defaults, plugin.id, t])

  if (!schema) return null

  const fields = Object.entries(schema.properties)
  const isValid = values !== null && fields.every(([name, field]) => validValue(values[name], field))

  function setValue(name: string, value: CastBoardPluginConfigValue) {
    setValues((current) => current ? { ...current, [name]: value } : current)
  }

  async function save() {
    if (!values || !isValid) return
    const overrides = Object.fromEntries(
      fields.flatMap(([name, field]) => Object.is(values[name], field.default) ? [] : [[name, values[name]]]),
    ) as CastBoardPluginConfigOverrides
    setSaving(true)
    try {
      await updateCastBoardPluginConfig(plugin.id, overrides)
      toast.success(t('castboard.plugins.configSaved'))
      onClose()
    } catch {
      toast.error(t('castboard.plugins.errors.config'))
    } finally {
      setSaving(false)
    }
  }

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4 backdrop-blur-sm animate-fade-in">
      <div aria-modal="true" className="max-h-[85vh] w-full max-w-lg overflow-y-auto rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in" role="dialog">
        <div className="flex items-start justify-between gap-4">
          <div className="flex items-center gap-3">
            <Settings className="h-5 w-5 text-[hsl(var(--accent))]" />
            <div>
              <div className="text-[16px] font-semibold text-[hsl(var(--text))]">
                {t('castboard.plugins.configTitle', { name: localizedText(plugin.name, language, plugin.id) })}
              </div>
              <p className="mt-1 text-[12px] text-[hsl(var(--muted))]">{t('castboard.plugins.configDescription')}</p>
            </div>
          </div>
          <button
            className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
            disabled={saving}
            onClick={onClose}
            title={t('common.close')}
            type="button"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <section className="mt-5 overflow-visible">
          {values === null && <div className="py-8 text-center text-[12px] text-[hsl(var(--muted))]">{t('common.loading')}</div>}
          {values !== null && (
            <div className="grid gap-5">
              {fields.map(([name, field]) => {
                const title = localizedText(field.title, language, name)
                const description = localizedText(field.description, language, '')
                const value = values[name]
                if (field.type === 'boolean') {
                  return (
                    <div className="flex items-center justify-between gap-4 py-1" key={name}>
                      <div className="min-w-0">
                        <div className="text-[13px] font-medium text-[hsl(var(--text))]">{title}</div>
                        {description && <div className="mt-0.5 text-[11px] text-[hsl(var(--muted))]">{description}</div>}
                      </div>
                      <Switch checked={Boolean(value)} onChange={(event) => setValue(name, event.target.checked)} />
                    </div>
                  )
                }
                return (
                  <div className="block" key={name}>
                    <div className="text-[13px] font-medium text-[hsl(var(--text))]">{title}</div>
                    {description && <div className="mt-0.5 text-[11px] text-[hsl(var(--muted))]">{description}</div>}
                    <div className="mt-2">
                      {field.type === 'string' && field.enum && (
                        <Select
                          ariaLabel={title}
                          onValueChange={(nextValue) => setValue(name, nextValue)}
                          options={field.enum.map((option) => ({
                            value: option,
                            label: localizedText(field.enumTitles?.[option], language, option),
                          }))}
                          value={String(value)}
                        />
                      )}
                      {field.type === 'string' && !field.enum && (
                        <Input
                          onChange={(event) => setValue(name, event.target.value)}
                          type={field.format === 'password' ? 'password' : 'text'}
                          value={String(value)}
                        />
                      )}
                      {(field.type === 'number' || field.type === 'integer') && (
                        <div className="flex items-center gap-3">
                          {field.minimum !== undefined && field.maximum !== undefined && (
                            <Slider
                              aria-label={title}
                              className="min-w-0 flex-1"
                              max={field.maximum}
                              min={field.minimum}
                              onChange={(event) => setValue(name, Number(event.target.value))}
                              step={field.type === 'integer' ? 1 : 'any'}
                              value={Number(value)}
                            />
                          )}
                          <Input
                            className="w-28"
                            max={field.maximum}
                            min={field.minimum}
                            onChange={(event) => setValue(name, Number(event.target.value))}
                            step={field.type === 'integer' ? 1 : 'any'}
                            type="number"
                            value={Number(value)}
                          />
                        </div>
                      )}
                    </div>
                  </div>
                )
              })}
            </div>
          )}
        </section>

        <div className="mt-6 flex justify-between gap-2">
          <Button disabled={!values || saving} onClick={() => setValues({ ...defaults })} variant="secondary">
            {t('castboard.plugins.resetDefaults')}
          </Button>
          <div className="flex gap-2">
            <Button disabled={saving} onClick={onClose} variant="secondary">{t('common.cancel')}</Button>
            <Button disabled={!isValid || saving} onClick={() => { void save() }}>
              {saving ? t('castboard.plugins.saving') : t('common.save')}
            </Button>
          </div>
        </div>
      </div>
    </div>,
    document.body,
  )
}
