import { AlertTriangle, Download, X } from 'lucide-react'
import ReactMarkdown from 'react-markdown'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'

import { openUpdateDownload } from '../lib/api'
import type { AppUpdateRelease } from '../lib/types'
import { Button } from './ui/button'

interface UpdateDialogProps {
  update: AppUpdateRelease | null
  required: boolean
  onClose: () => void
}

export function UpdateDialog({ update, required, onClose }: UpdateDialogProps) {
  const { t } = useTranslation()

  if (!update) {
    return null
  }

  const asset = update.assets[0]
  const notes = update.releaseNotes.trim()
  const description = notes || t('updates.description')

  async function openDownload() {
    if (!asset) {
      return
    }
    try {
      await openUpdateDownload(asset.downloadUrl)
      if (!required) {
        onClose()
      }
    } catch {
      toast.error(t('common.requestFailed'))
    }
  }

  return (
    <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/45 px-4 backdrop-blur-sm">
      <div className="flex h-[34rem] max-h-[calc(100vh-2rem)] w-full max-w-xl flex-col rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
        <div className="flex items-start justify-between gap-4">
          <div className="flex min-w-0 items-start gap-3">
            <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-[hsl(var(--panel-2))]">
              {required ? (
                <AlertTriangle className="h-4 w-4 text-[hsl(var(--warning))]" />
              ) : (
                <Download className="h-4 w-4 text-[hsl(var(--accent))]" />
              )}
            </div>
            <div className="min-w-0">
              <div className="text-[15px] font-semibold text-[hsl(var(--text))]">
                {t('updates.available', { version: update.version })}
              </div>
              <div className="mt-1 text-[12px] text-[hsl(var(--muted))]">
                {required ? t('updates.required') : t('updates.description')}
              </div>
            </div>
          </div>
          {!required && (
            <button
              className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
              onClick={onClose}
              title={t('common.close')}
              type="button"
            >
              <X className="h-4 w-4" />
            </button>
          )}
        </div>

        <div className="mt-5 min-h-0 flex-1 overflow-y-auto rounded-lg border bg-[hsl(var(--panel-2))] px-4 py-3 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
          <ReactMarkdown
            components={{
              h1: ({ children }) => <h1 className="mb-3 text-lg font-semibold text-[hsl(var(--text))]">{children}</h1>,
              h2: ({ children }) => <h2 className="mb-2 mt-5 text-base font-semibold text-[hsl(var(--text))] first:mt-0">{children}</h2>,
              h3: ({ children }) => <h3 className="mb-2 mt-4 text-sm font-semibold text-[hsl(var(--text))]">{children}</h3>,
              p: ({ children }) => <p className="mb-3 last:mb-0">{children}</p>,
              ul: ({ children }) => <ul className="mb-3 list-disc space-y-1 pl-5 last:mb-0">{children}</ul>,
              ol: ({ children }) => <ol className="mb-3 list-decimal space-y-1 pl-5 last:mb-0">{children}</ol>,
              li: ({ children }) => <li>{children}</li>,
              a: ({ children, href }) => (
                <a className="text-[hsl(var(--accent))] underline underline-offset-2" href={href} rel="noreferrer" target="_blank">
                  {children}
                </a>
              ),
              code: ({ children }) => <code className="rounded bg-[hsl(var(--panel))] px-1 py-0.5 font-mono text-[12px] text-[hsl(var(--text))]">{children}</code>,
              pre: ({ children }) => <pre className="mb-3 overflow-x-auto rounded bg-[hsl(var(--panel))] p-3 text-[12px] text-[hsl(var(--text))]">{children}</pre>,
              blockquote: ({ children }) => <blockquote className="mb-3 border-l-2 border-[hsl(var(--border))] pl-3 text-[hsl(var(--muted))]">{children}</blockquote>,
            }}
          >
            {description}
          </ReactMarkdown>
        </div>

        <div className="mt-6 flex justify-end gap-2">
          {!required && (
            <Button onClick={onClose} variant="secondary">
              {t('updates.later')}
            </Button>
          )}
          {asset && (
            <Button onClick={openDownload} variant="primary">
              <Download className="h-4 w-4" />
              {t('updates.download')}
            </Button>
          )}
        </div>
      </div>
    </div>
  )
}
