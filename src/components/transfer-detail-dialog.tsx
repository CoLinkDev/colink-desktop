import { createPortal } from 'react-dom'
import { X } from 'lucide-react'
import { useTranslation } from 'react-i18next'

import type { FileTransferRecord } from '../lib/types'
import { formatBytes } from '../lib/utils'
import { Button } from './ui/button'

interface TransferDetailDialogProps {
  transfer: FileTransferRecord
  deviceName: string | null
  onClose: () => void
}

interface DetailRowData {
  label: string
  value: string
  mono?: boolean
}

export function TransferDetailDialog({ transfer, deviceName, onClose }: TransferDetailDialogProps) {
  const { t } = useTranslation()

  const separatorIndex = transfer.checksum.indexOf(':')
  const algorithm = separatorIndex >= 0 ? transfer.checksum.slice(0, separatorIndex).toLowerCase() : ''
  const hashContent = separatorIndex >= 0 ? transfer.checksum.slice(separatorIndex + 1) : ''
  const algorithmLabel = algorithm === 'none'
    ? t('common.none')
    : algorithm === 'blake3'
      ? 'BLAKE3'
      : algorithm === 'sha256'
        ? 'SHA-256'
        : algorithm || '?'
  const hashLabel = algorithm === 'none' ? t('common.none') : hashContent
  const statusLabel = t(`transfers.status.${transfer.status}`, { defaultValue: transfer.status })
  const timeText = new Date(transfer.createdAt).toLocaleString()
  const directionLabel = transfer.direction === 'outbound'
    ? t('transfers.directionSend')
    : t('transfers.directionReceive')
  const routeLabel = transfer.route === 'lan'
    ? t('transfers.routeLan')
    : transfer.route === 'cloud'
      ? t('transfers.routeCloud')
      : transfer.route || '-'
  const localPath = transfer.finalPath || transfer.tempPath

  const rows: DetailRowData[] = [
    { label: t('transfers.details.status'), value: statusLabel },
    ...(transfer.error
      ? [{ label: t('transfers.details.error'), value: transfer.error }]
      : []),
    { label: t('transfers.details.time'), value: timeText },
    { label: t('transfers.details.fileSize'), value: formatBytes(transfer.fileSize) },
    { label: t('transfers.details.localLocation'), value: localPath || t('common.none'), mono: true },
    { label: t('transfers.details.hashAlgorithm'), value: algorithmLabel },
    { label: t('transfers.details.hash'), value: hashLabel, mono: true },
    { label: t('transfers.details.direction'), value: directionLabel },
    { label: t('transfers.details.route'), value: routeLabel },
    { label: t('transfers.details.device'), value: deviceName || transfer.deviceId },
  ]

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm animate-fade-in">
      <div className="flex max-h-[82vh] w-full max-w-xl flex-col rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
        <div className="flex shrink-0 items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="break-words text-[16px] font-semibold text-[hsl(var(--text))]">
              {transfer.fileName || t('common.none')}
            </div>
            <div className="mt-0.5 text-[12px] text-[hsl(var(--muted))]">{statusLabel}</div>
          </div>
          <button
            className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
            onClick={onClose}
            title={t('common.close')}
            type="button"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="mt-5 min-h-0 flex-1 overflow-y-auto pr-1">
          <div className="divide-y">
            {rows.map((row) => (
              <DetailRow key={row.label} row={row} />
            ))}
          </div>
        </div>

        <div className="mt-6 flex shrink-0 justify-end gap-2">
          <Button onClick={onClose} variant="primary">
            {t('common.close')}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  )
}

function DetailRow({ row }: { row: DetailRowData }) {
  return (
    <div className="grid gap-1 py-2.5 md:grid-cols-[160px_minmax(0,1fr)] md:gap-4">
      <div className="text-[12px] text-[hsl(var(--muted))]">{row.label}</div>
      <div
        className={
          row.mono
            ? 'break-all font-mono text-[12px] leading-relaxed text-[hsl(var(--text-secondary))]'
            : 'break-words text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]'
        }
      >
        {row.value}
      </div>
    </div>
  )
}
