import type { FormEvent, ReactNode } from 'react'
import { useState } from 'react'
import { z } from 'zod'
import { toast } from 'sonner'

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

function SettingsForm({ settings, onSave, onPickDownloadDirectory }: SettingsFormProps) {
  const [form, setForm] = useState<AppSettings>(settings)

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const parsed = settingsSchema.safeParse(form)
    if (!parsed.success) {
      toast.error(parsed.error.issues[0]?.message ?? '设置不完整')
      return
    }
    try {
      await onSave(parsed.data)
      toast.success('设置已保存')
    } catch (e) {
      toast.error(readErrorMessage(e))
    }
  }

  return (
    <div className="max-w-2xl animate-fade-in">
      <form id="settings-form" className="space-y-6" onSubmit={handleSubmit}>
        <Section title="网络">
          <Field label="Server URL" tip="桌面端请求 API 的地址">
            <Input onChange={(e) => setForm((c) => ({ ...c, serverUrl: e.target.value }))} value={form.serverUrl} />
          </Field>
          <Field label="下载路径" tip="接收文件时使用的本地目录">
            <div className="flex gap-2">
              <Input onChange={(e) => setForm((c) => ({ ...c, downloadPath: e.target.value }))} value={form.downloadPath} />
              <Button
                onClick={async () => { const p = await onPickDownloadDirectory(); if (p) setForm((c) => ({ ...c, downloadPath: p })) }}
                type="button" variant="secondary" className="shrink-0"
              >选择</Button>
            </div>
          </Field>
        </Section>

        <Section title="行为">
          <SwitchRow label="开机启动" checked={form.autoStart} onChange={(v) => setForm((c) => ({ ...c, autoStart: v }))} />
          <SwitchRow label="启动时最小化" checked={form.startMinimized} onChange={(v) => setForm((c) => ({ ...c, startMinimized: v }))} />
          <SwitchRow label="局域网发现" checked={form.lanDiscovery} onChange={(v) => setForm((c) => ({ ...c, lanDiscovery: v }))} />
          <SwitchRow label="通知" checked={form.notifications} onChange={(v) => setForm((c) => ({ ...c, notifications: v }))} />
        </Section>
      </form>
    </div>
  )
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="rounded-xl border bg-[hsl(var(--panel))] p-5">
      <div className="text-[11px] font-medium uppercase tracking-widest text-[hsl(var(--muted))]">{title}</div>
      <div className="mt-5 grid gap-5">{children}</div>
    </section>
  )
}

function Field({ label, tip, children }: { label: string; tip: string; children: ReactNode }) {
  return (
    <label className="block">
      <div className="text-[13px] font-medium">{label}</div>
      <div className="mt-0.5 text-[11px] text-[hsl(var(--muted))]">{tip}</div>
      <div className="mt-2">{children}</div>
    </label>
  )
}

function SwitchRow({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <div className="flex items-center justify-between py-1">
      <span className="text-[13px] font-medium">{label}</span>
      <Switch checked={checked} onChange={(e) => onChange(e.target.checked)} />
    </div>
  )
}
