import { Paperclip, Send, X } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { useSearchParams } from 'react-router-dom'

import { Button } from '../components/ui/button'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { formatBytes, formatTimestamp } from '../lib/utils'

export function MessagesPage() {
  const [searchParams, setSearchParams] = useSearchParams()
  const {
    device,
    devices,
    messages,
    transfers,
    sendText,
    pickFiles,
    sendFiles,
    cancelTransfer,
  } = useAppState()
  const [selectedDeviceId, setSelectedDeviceId] = useState(searchParams.get('deviceId') ?? '')
  const [text, setText] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const targetDevices = useMemo(
    () => devices.filter((item) => item.deviceId !== device?.deviceId),
    [device?.deviceId, devices],
  )

  useEffect(() => {
    if (selectedDeviceId && targetDevices.some((item) => item.deviceId === selectedDeviceId)) {
      return
    }

    const nextDeviceId = searchParams.get('deviceId') ?? targetDevices[0]?.deviceId ?? ''
    setSelectedDeviceId(nextDeviceId)
  }, [searchParams, selectedDeviceId, targetDevices])

  const selectedDevice = targetDevices.find((item) => item.deviceId === selectedDeviceId) ?? null

  const conversation = useMemo(
    () =>
      messages
        .filter((item) => item.deviceId === selectedDeviceId)
        .sort((left, right) => left.createdAt - right.createdAt),
    [messages, selectedDeviceId],
  )

  const transferItems = useMemo(
    () =>
      transfers.filter((item) =>
        selectedDeviceId ? item.deviceId === selectedDeviceId : true,
      ),
    [selectedDeviceId, transfers],
  )

  async function handleSendText() {
    if (!selectedDeviceId) {
      setError('先选目标设备')
      return
    }

    const nextText = text.trim()
    if (!nextText) {
      setError('消息不能为空')
      return
    }

    setSubmitting(true)
    setError(null)

    try {
      await sendText({
        deviceId: selectedDeviceId,
        text: nextText,
      })
      setText('')
    } catch (requestError) {
      setError(readErrorMessage(requestError))
    } finally {
      setSubmitting(false)
    }
  }

  async function handleSendFiles() {
    if (!selectedDeviceId) {
      setError('先选目标设备')
      return
    }

    setSubmitting(true)
    setError(null)

    try {
      const paths = await pickFiles(true)
      if (paths.length === 0) {
        return
      }

      await sendFiles({
        deviceId: selectedDeviceId,
        paths,
      })
    } catch (requestError) {
      setError(readErrorMessage(requestError))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="grid gap-6 xl:grid-cols-[320px_minmax(0,1fr)]">
      <aside className="surface rounded-lg border border-[hsl(var(--border))] p-5">
        <div className="text-sm text-[hsl(var(--muted))]">目标设备</div>
        <div className="mt-3 grid gap-3">
          {targetDevices.length === 0 ? (
            <div className="rounded-lg border border-dashed border-[hsl(var(--border))] px-4 py-6 text-sm text-[hsl(var(--muted))]">
              还没有其它设备。
            </div>
          ) : (
            targetDevices.map((item) => (
              <button
                className={
                  item.deviceId === selectedDeviceId
                    ? 'rounded-lg border border-[hsl(var(--accent)/0.55)] bg-[hsl(var(--accent)/0.12)] px-4 py-4 text-left'
                    : 'rounded-lg border border-[hsl(var(--border))] px-4 py-4 text-left transition hover:bg-[hsl(var(--panel-2))]'
                }
                key={item.deviceId}
                onClick={() => {
                  setSelectedDeviceId(item.deviceId)
                  setSearchParams({ deviceId: item.deviceId })
                }}
                type="button"
              >
                <div className="flex items-center justify-between gap-3">
                  <div className="text-sm font-medium text-[hsl(var(--text))]">{item.name}</div>
                  <div
                    className={
                      item.online
                        ? 'text-xs text-[hsl(var(--accent))]'
                        : 'text-xs text-[hsl(var(--muted))]'
                    }
                  >
                    {item.online ? '在线' : '离线'}
                  </div>
                </div>
                <div className="mt-2 text-xs text-[hsl(var(--muted))]">
                  {item.type} · {item.activeRoute ?? 'cloud'}
                </div>
              </button>
            ))
          )}
        </div>
      </aside>

      <div className="space-y-6">
        <section className="surface rounded-lg border border-[hsl(var(--border))] p-5">
          <div className="flex flex-wrap items-center justify-between gap-4">
            <div>
              <div className="text-sm text-[hsl(var(--muted))]">文本消息</div>
              <div className="mt-1 text-xl font-semibold">
                {selectedDevice?.name ?? '未选择设备'}
              </div>
            </div>
            <div className="flex gap-3">
              <Button
                disabled={submitting || !selectedDeviceId}
                onClick={() => void handleSendFiles()}
                variant="secondary"
              >
                <Paperclip className="h-4 w-4" />
                发送文件
              </Button>
            </div>
          </div>

          <div className="mt-5 rounded-lg border border-[hsl(var(--border))] bg-[hsl(var(--panel-2))] p-4">
            <textarea
              className="min-h-28 w-full resize-none bg-transparent text-sm outline-none"
              onChange={(event) => setText(event.target.value)}
              placeholder="输入要发的内容"
              value={text}
            />
            <div className="mt-4 flex items-center justify-between gap-3">
              <div className="text-xs text-[hsl(var(--muted))]">{text.length}/10000</div>
              <Button disabled={submitting || !selectedDeviceId} onClick={() => void handleSendText()}>
                <Send className="h-4 w-4" />
                发送
              </Button>
            </div>
          </div>

          {error && <div className="mt-4 text-sm text-[hsl(var(--danger))]">{error}</div>}
        </section>

        <section className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_360px]">
          <div className="surface rounded-lg border border-[hsl(var(--border))] p-5">
            <div className="text-sm text-[hsl(var(--muted))]">历史消息</div>
            <div className="mt-4 space-y-3">
              {conversation.length === 0 ? (
                <div className="rounded-lg border border-dashed border-[hsl(var(--border))] px-4 py-8 text-sm text-[hsl(var(--muted))]">
                  这台设备还没有消息记录。
                </div>
              ) : (
                conversation.map((item) => (
                  <div
                    className={
                      item.direction === 'outbound'
                        ? 'ml-auto max-w-[80%] rounded-2xl rounded-br-md bg-[hsl(var(--accent)/0.18)] px-4 py-3'
                        : 'max-w-[80%] rounded-2xl rounded-bl-md border border-[hsl(var(--border))] bg-[hsl(var(--panel-2))] px-4 py-3'
                    }
                    key={item.messageId}
                  >
                    <div className="whitespace-pre-wrap text-sm text-[hsl(var(--text))]">
                      {item.text}
                    </div>
                    <div className="mt-2 text-[11px] text-[hsl(var(--muted))]">
                      {formatTimestamp(item.createdAt)} · {item.route}
                    </div>
                  </div>
                ))
              )}
            </div>
          </div>

          <div className="surface rounded-lg border border-[hsl(var(--border))] p-5">
            <div className="text-sm text-[hsl(var(--muted))]">文件传输</div>
            <div className="mt-4 space-y-3">
              {transferItems.length === 0 ? (
                <div className="rounded-lg border border-dashed border-[hsl(var(--border))] px-4 py-8 text-sm text-[hsl(var(--muted))]">
                  还没有传输记录。
                </div>
              ) : (
                transferItems.map((item) => {
                  const progress = item.fileSize > 0 ? item.transferredBytes / item.fileSize : 0
                  const active =
                    item.status === 'offered' ||
                    item.status === 'sending' ||
                    item.status === 'receiving'

                  return (
                    <article
                      className="rounded-lg border border-[hsl(var(--border))] bg-[hsl(var(--panel-2))] p-4"
                      key={item.fileId}
                    >
                      <div className="flex items-start justify-between gap-3">
                        <div>
                          <div className="text-sm font-medium text-[hsl(var(--text))]">
                            {item.fileName}
                          </div>
                          <div className="mt-1 text-xs text-[hsl(var(--muted))]">
                            {item.direction === 'outbound' ? '发送' : '接收'} ·{' '}
                            {formatBytes(item.fileSize)}
                          </div>
                        </div>
                        {active && (
                          <button
                            className="rounded-md border border-[hsl(var(--border))] p-2 text-[hsl(var(--muted))] transition hover:text-[hsl(var(--text))]"
                            onClick={() => void cancelTransfer(item.fileId)}
                            type="button"
                          >
                            <X className="h-4 w-4" />
                          </button>
                        )}
                      </div>

                      <div className="mt-4 h-2 overflow-hidden rounded-full bg-[hsl(var(--border))]">
                        <div
                          className="h-full rounded-full bg-[hsl(var(--accent))] transition-all"
                          style={{ width: `${Math.max(6, Math.min(100, progress * 100))}%` }}
                        />
                      </div>

                      <div className="mt-3 flex items-center justify-between gap-3 text-xs text-[hsl(var(--muted))]">
                        <span>{item.status}</span>
                        <span>
                          {formatBytes(item.transferredBytes)} / {formatBytes(item.fileSize)}
                        </span>
                      </div>
                      {item.error && (
                        <div className="mt-2 text-xs text-[hsl(var(--danger))]">{item.error}</div>
                      )}
                    </article>
                  )
                })
              )}
            </div>
          </div>
        </section>
      </div>
    </div>
  )
}
