import type { FormEvent, ReactNode } from 'react'
import { useState } from 'react'
import { z } from 'zod'

import { Button } from '../components/ui/button'
import { Input } from '../components/ui/input'
import { Switch } from '../components/ui/switch'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import type { AppSettings } from '../lib/types'

const settingsSchema = z.object({
  serverUrl: z.string().url('Server URL 不合法'),
  autoStart: z.boolean(),
  startMinimized: z.boolean(),
  lanDiscovery: z.boolean(),
  downloadPath: z.string().min(1, '下载路径不能为空'),
  notifications: z.boolean(),
})

export function SettingsPage() {
  const { settings, saveSettings, pickDownloadDirectory } = useAppState()

  return (
    <SettingsForm
      key={JSON.stringify(settings)}
      onPickDownloadDirectory={pickDownloadDirectory}
      onSave={saveSettings}
      settings={settings}
    />
  )
}

interface SettingsFormProps {
  settings: AppSettings
  onSave: (settings: AppSettings) => Promise<void>
  onPickDownloadDirectory: () => Promise<string | null>
}

function SettingsForm({
  settings,
  onSave,
  onPickDownloadDirectory,
}: SettingsFormProps) {
  const [form, setForm] = useState<AppSettings>(settings)
  const [submitting, setSubmitting] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setError(null)
    setMessage(null)

    const parsed = settingsSchema.safeParse(form)

    if (!parsed.success) {
      setError(parsed.error.issues[0]?.message ?? '设置不完整')
      return
    }

    setSubmitting(true)

    try {
      await onSave(parsed.data)
      setMessage('设置已保存')
    } catch (requestError) {
      setError(readErrorMessage(requestError))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="max-w-4xl">
      <div>
        <div className="text-lg font-semibold">设置</div>
        <div className="mt-1 text-sm text-[hsl(var(--muted))]">
          网络、后台行为和接收目录都在这里控制。
        </div>
      </div>

      <form className="mt-8 space-y-8" onSubmit={handleSubmit}>
        <section className="surface rounded-lg border border-[hsl(var(--border))] p-6">
          <div className="text-sm font-medium">网络</div>
          <div className="mt-6 grid gap-6">
            <Field label="Server URL" tip="桌面端请求 API 的地址。">
              <Input
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    serverUrl: event.target.value,
                  }))
                }
                value={form.serverUrl}
              />
            </Field>

            <Field label="下载路径" tip="接收文件时使用的本地目录。">
              <div className="flex gap-3">
                <Input
                  onChange={(event) =>
                    setForm((current) => ({
                      ...current,
                      downloadPath: event.target.value,
                    }))
                  }
                  value={form.downloadPath}
                />
                <Button
                  onClick={async () => {
                    const nextPath = await onPickDownloadDirectory()
                    if (!nextPath) {
                      return
                    }

                    setForm((current) => ({
                      ...current,
                      downloadPath: nextPath,
                    }))
                  }}
                  type="button"
                  variant="secondary"
                >
                  选择目录
                </Button>
              </div>
            </Field>
          </div>
        </section>

        <section className="surface rounded-lg border border-[hsl(var(--border))] p-6">
          <div className="text-sm font-medium">行为</div>
          <div className="mt-6 grid gap-4">
            <SwitchRow
              checked={form.autoStart}
              description="保存后会写入系统启动项。"
              label="开机启动"
              onChange={(checked) =>
                setForm((current) => ({
                  ...current,
                  autoStart: checked,
                }))
              }
            />
            <SwitchRow
              checked={form.startMinimized}
              description="下次启动会按托盘模式打开。"
              label="启动时最小化"
              onChange={(checked) =>
                setForm((current) => ({
                  ...current,
                  startMinimized: checked,
                }))
              }
            />
            <SwitchRow
              checked={form.lanDiscovery}
              description="保存后会更新局域网发现开关。"
              label="局域网发现"
              onChange={(checked) =>
                setForm((current) => ({
                  ...current,
                  lanDiscovery: checked,
                }))
              }
            />
            <SwitchRow
              checked={form.notifications}
              description="保存后会影响系统通知。"
              label="通知"
              onChange={(checked) =>
                setForm((current) => ({
                  ...current,
                  notifications: checked,
                }))
              }
            />
          </div>
        </section>

        {(message || error) && (
          <div
            className={
              error
                ? 'rounded-lg border border-[hsl(var(--danger)/0.5)] bg-[hsl(var(--danger)/0.12)] px-4 py-3 text-sm text-[hsl(var(--text))]'
                : 'rounded-lg border border-[hsl(var(--accent)/0.5)] bg-[hsl(var(--accent)/0.12)] px-4 py-3 text-sm text-[hsl(var(--text))]'
            }
          >
            {error ?? message}
          </div>
        )}

        <Button disabled={submitting} type="submit">
          {submitting ? '正在保存' : '保存设置'}
        </Button>
      </form>
    </div>
  )
}

interface FieldProps {
  label: string
  tip: string
  children: ReactNode
}

function Field({ label, tip, children }: FieldProps) {
  return (
    <label className="block">
      <div className="text-sm text-[hsl(var(--text))]">{label}</div>
      <div className="mt-1 text-sm text-[hsl(var(--muted))]">{tip}</div>
      <div className="mt-3">{children}</div>
    </label>
  )
}

interface SwitchRowProps {
  label: string
  description: string
  checked: boolean
  onChange: (checked: boolean) => void
}

function SwitchRow({
  label,
  description,
  checked,
  onChange,
}: SwitchRowProps) {
  return (
    <div className="flex items-center justify-between gap-4 rounded-lg border border-[hsl(var(--border))] px-4 py-3">
      <div>
        <div className="text-sm text-[hsl(var(--text))]">{label}</div>
        <div className="mt-1 text-sm text-[hsl(var(--muted))]">{description}</div>
      </div>

      <Switch checked={checked} onChange={(event) => onChange(event.target.checked)} />
    </div>
  )
}
