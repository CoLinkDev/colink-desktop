import {
  AlertCircle,
  Archive,
  ArrowDown,
  ArrowUp,
  CheckCircle2,
  ExternalLink,
  File,
  FileText,
  FolderOpen,
  Image,
  Info,
  LoaderCircle,
  Music,
  Video,
  X,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'

import { Button } from './ui/button'
import { cn, formatBytes, formatTimestamp } from '../lib/utils'
import type { FileOfferRequest, FileTransferRecord } from '../lib/types'

interface TransferBubbleCardProps {
  transfer: FileTransferRecord
  speed?: number | null
  language?: string
  onCancel: (fileId: string) => void
  onOpen: (fileId: string) => void
  onReveal: (fileId: string) => void
  onDetails: (transfer: FileTransferRecord) => void
}

function fileIcon(fileName: string) {
  const extension = fileName.split('.').pop()?.toLowerCase() ?? ''
  if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg'].includes(extension)) return Image
  if (['mp3', 'wav', 'flac', 'm4a', 'ogg'].includes(extension)) return Music
  if (['mp4', 'mov', 'mkv', 'webm', 'avi'].includes(extension)) return Video
  if (['zip', '7z', 'rar', 'tar', 'gz', 'bz2'].includes(extension)) return Archive
  if (['txt', 'md', 'pdf', 'doc', 'docx', 'xls', 'xlsx', 'ppt', 'pptx'].includes(extension)) return FileText
  return File
}

export function TransferBubbleCard({
  transfer,
  speed,
  language,
  onCancel,
  onOpen,
  onReveal,
  onDetails,
}: TransferBubbleCardProps) {
  const { t } = useTranslation()
  const Icon = fileIcon(transfer.fileName)
  const active = ['offered', 'sending', 'receiving'].includes(transfer.status)
  const inFlight = transfer.status === 'sending' || transfer.status === 'receiving'
  const completed = transfer.status === 'completed'
  const failed = ['failed', 'cancelled', 'rejected'].includes(transfer.status)
  const progress = transfer.fileSize > 0
    ? Math.max(0, Math.min(1, transfer.transferredBytes / transfer.fileSize))
    : 0
  const statusLabel = t(`transfers.status.${transfer.status}`, { defaultValue: transfer.status })
  const routeLabel = transfer.route === 'lan'
    ? t('transfers.routeLan')
    : transfer.route === 'cloud'
      ? t('transfers.routeCloud')
      : transfer.route || '-'
  const canOpen = transfer.direction === 'inbound' && completed && Boolean(transfer.finalPath)

  return (
    <div className={cn(
      'max-w-[min(88%,560px)] rounded-2xl border px-3.5 py-3 shadow-sm',
      transfer.direction === 'outbound'
        ? 'ml-auto rounded-br-md border-[hsl(var(--text)/0.08)] bg-[hsl(var(--text)/0.07)]'
        : 'rounded-bl-md border-[hsl(var(--border))] bg-[hsl(var(--panel-2)/0.45)]',
    )}>
      <div className="flex items-start gap-3">
        <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-[hsl(var(--panel-2))] text-[hsl(var(--text-secondary))]">
          <Icon className="h-4 w-4" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-start justify-between gap-2">
            <div className="min-w-0">
              <div className="truncate text-[13px] font-semibold text-[hsl(var(--text))]" title={transfer.fileName}>
                {transfer.fileName}
              </div>
              <div className="mt-1 flex items-center gap-1.5 text-[11px] text-[hsl(var(--muted))]">
                {transfer.direction === 'outbound' ? <ArrowUp className="h-3 w-3" /> : <ArrowDown className="h-3 w-3" />}
                <span>{formatBytes(transfer.fileSize)}</span>
                <span>·</span>
                <span>{routeLabel}</span>
              </div>
            </div>
            <Button aria-label={t('transfers.detailsTitle')} className="h-7 w-7 shrink-0 px-0" onClick={() => onDetails(transfer)} size="sm" title={t('transfers.detailsTitle')} variant="ghost">
              <Info className="h-3.5 w-3.5" />
            </Button>
          </div>

          <div className="mt-3 flex items-center justify-between gap-2 text-[11px]">
            <span className={cn(
              'truncate font-medium',
              completed ? 'text-[hsl(var(--success))]' : failed ? 'text-[hsl(var(--danger))]' : 'text-[hsl(var(--text-secondary))]',
            )} title={failed ? (transfer.error || statusLabel) : statusLabel}>
              {failed ? (transfer.error || statusLabel) : statusLabel}
            </span>
            <span className="shrink-0 text-[hsl(var(--muted))]">
              {formatBytes(transfer.transferredBytes)} / {formatBytes(transfer.fileSize)}
              {inFlight && speed !== null && speed !== undefined ? ` · ${formatBytes(Math.max(0, speed))}/s` : ''}
            </span>
          </div>
          <div className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-[hsl(var(--border))]">
            <div
              className={cn(
                'h-full rounded-full transition-all duration-300',
                failed ? 'bg-[hsl(var(--danger))]' : completed ? 'bg-[hsl(var(--success))]' : 'bg-[hsl(var(--text-secondary))]',
              )}
              style={{ width: `${Math.max(active || completed ? 4 : 0, progress * 100)}%` }}
            />
          </div>

          <div className="mt-2 flex items-center justify-between gap-2">
            <span className="text-[10px] text-[hsl(var(--muted))]">{formatTimestamp(transfer.updatedAt, language)}</span>
            <div className="flex items-center gap-1">
              {canOpen && (
                <>
                  <Button aria-label={t('transfers.openFile')} className="h-7 w-7 px-0" onClick={() => onOpen(transfer.fileId)} size="sm" title={t('transfers.openFile')} variant="ghost"><ExternalLink className="h-3.5 w-3.5" /></Button>
                  <Button aria-label={t('transfers.revealFile')} className="h-7 w-7 px-0" onClick={() => onReveal(transfer.fileId)} size="sm" title={t('transfers.revealFile')} variant="ghost"><FolderOpen className="h-3.5 w-3.5" /></Button>
                </>
              )}
              {active && (
                <Button aria-label={t('transfers.cancelTitle')} className="h-7 w-7 px-0" onClick={() => onCancel(transfer.fileId)} size="sm" title={t('transfers.cancelTitle')} variant="ghost">
                  {inFlight ? <X className="h-3.5 w-3.5" /> : <LoaderCircle className="h-3.5 w-3.5" />}
                </Button>
              )}
              {completed && <CheckCircle2 className="ml-1 h-4 w-4 text-[hsl(var(--success))]" />}
              {failed && <AlertCircle className="ml-1 h-4 w-4 text-[hsl(var(--danger))]" />}
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

interface FileOfferBubbleProps {
  request: FileOfferRequest
  timestamp: number
  acting: boolean
  onRespond: (request: FileOfferRequest, accepted: boolean) => void
}

export function FileOfferBubble({ request, timestamp, acting, onRespond }: FileOfferBubbleProps) {
  const { t, i18n } = useTranslation()
  return (
    <div className="max-w-[min(88%,560px)] rounded-2xl rounded-bl-md border border-[hsl(var(--border))] bg-[hsl(var(--panel-2)/0.45)] px-3.5 py-3 shadow-sm">
      <div className="flex items-start gap-3">
        <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-[hsl(var(--panel-2))] text-[hsl(var(--accent))]"><ArrowDown className="h-4 w-4" /></div>
        <div className="min-w-0 flex-1">
          <div className="text-[12px] font-medium text-[hsl(var(--text-secondary))]">{t('fileOffers.description', { name: request.deviceName || request.deviceId })}</div>
          <div className="mt-2 truncate text-[13px] font-semibold text-[hsl(var(--text))]" title={request.fileName}>{request.fileName}</div>
          <div className="mt-1 text-[11px] text-[hsl(var(--muted))]">{formatBytes(request.fileSize)}</div>
          <div className="mt-3 flex items-center justify-between gap-2">
            <span className="text-[10px] text-[hsl(var(--muted))]">{formatTimestamp(timestamp, i18n.language)}</span>
            <div className="flex gap-2">
              <Button disabled={acting} onClick={() => onRespond(request, false)} size="sm" variant="secondary">{t('common.cancel')}</Button>
              <Button disabled={acting} onClick={() => onRespond(request, true)} size="sm">{acting ? <LoaderCircle className="h-3.5 w-3.5 animate-spin" /> : null}{t('fileOffers.accept')}</Button>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}
