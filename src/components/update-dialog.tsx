import { AlertTriangle, Download, X } from 'lucide-react'
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
      <div className="w-full max-w-md rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
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

        <div className="mt-5 max-h-60 overflow-auto whitespace-pre-wrap rounded-lg border bg-[hsl(var(--panel-2))] px-4 py-3 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
          {description}
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
