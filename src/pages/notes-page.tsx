import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import {
  ArrowLeft,
  FileText,
  Image,
  Paperclip,
  Pencil,
  Plus,
  RefreshCw,
  Save,
  Search,
  Tag as TagIcon,
  Trash2,
  TriangleAlert,
  X,
} from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'

import { Button } from '../components/ui/button'
import { Input } from '../components/ui/input'
import { RichEditor, type RichEditorHandle } from '../components/notes/rich-editor'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { hasHttpStatus, hasProtocolCode } from '../lib/command-error'
import {
  notesAttachmentsOpen,
  notesAttachmentsDelete,
  notesAttachmentsStage,
  notesDelete,
  notesNewId,
  notesResolveConflict,
  notesTagsCreate,
  notesTagsDelete,
  notesTagsRename,
  notesUpsert,
  notesAttachmentsList,
} from '../lib/api'
import {
  defaultSettings,
  type NoteAttachmentRecord,
  type NoteRecord,
} from '../lib/types'
import { cn, formatTimestamp } from '../lib/utils'

interface DraftState {
  id: string | null
  title: string
  markdown: string
  tagIds: string[]
  attachmentIds: string[]
}

function sameDraft(left: DraftState | null, right: DraftState | null): boolean {
  if (!left || !right) return left === right
  return left.id === right.id && left.title === right.title && left.markdown === right.markdown &&
    [...left.tagIds].sort().join('\n') === [...right.tagIds].sort().join('\n') &&
    [...left.attachmentIds].sort().join('\n') === [...right.attachmentIds].sort().join('\n')
}

function NotesDialog({
  title,
  description,
  children,
  confirmLabel,
  width = 'sm',
  danger = false,
  confirmDisabled = false,
  onConfirm,
  onClose,
}: {
  title: string
  description?: string
  children?: ReactNode
  confirmLabel?: string
  width?: 'sm' | 'md' | 'lg'
  danger?: boolean
  confirmDisabled?: boolean
  onConfirm?: () => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4 backdrop-blur-sm animate-fade-in">
      <div
        role="dialog"
        aria-modal="true"
        aria-label={title}
        className={cn(
          'w-full rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in',
          width === 'sm' ? 'max-w-sm' : width === 'md' ? 'max-w-md' : 'max-w-lg',
        )}
      >
        <div className="text-[16px] font-semibold text-[hsl(var(--text))]">{title}</div>
        {description && (
          <p className="mt-2 text-[13px] leading-relaxed text-[hsl(var(--text-secondary))]">
            {description}
          </p>
        )}
        {children && <div className="mt-5">{children}</div>}
        <div className="mt-6 flex shrink-0 justify-end gap-2">
          {onConfirm ? (
            <>
              <Button variant="secondary" onClick={onClose}>{t('common.cancel')}</Button>
              <Button
                variant={danger ? 'danger' : 'primary'}
                disabled={confirmDisabled}
                onClick={onConfirm}
              >
                {confirmLabel ?? t('common.confirm')}
              </Button>
            </>
          ) : (
            <Button onClick={onClose}>{t('common.close')}</Button>
          )}
        </div>
      </div>
    </div>,
    document.body,
  )
}

function mergeIdSet(ancestor: string[], local: string[], cloud: string[]): string[] {
  const result = new Set<string>()
  for (const id of [...ancestor, ...local, ...cloud]) {
    const existed = ancestor.includes(id)
    const inLocal = local.includes(id)
    const inCloud = cloud.includes(id)
    if ((inLocal && inCloud) || (inLocal !== inCloud && !existed)) {
      result.add(id)
    }
  }
  return [...result].sort()
}

function conflictDraftFromNote(note: NoteRecord): DraftState {
  return {
    id: note.id,
    title: note.title,
    markdown: note.markdown,
    tagIds: mergeIdSet(note.ancestorTagIds, note.tagIds, note.conflictTagIds ?? []),
    attachmentIds: mergeIdSet(
      note.ancestorAttachmentIds,
      note.attachmentIds,
      note.conflictAttachmentIds ?? [],
    ),
  }
}

function draftFromNote(note: NoteRecord): DraftState {
  return {
    id: note.id,
    title: note.title,
    markdown: note.markdown,
    tagIds: [...note.tagIds],
    attachmentIds: [...note.attachmentIds],
  }
}

export function NotesPage() {
  const { t } = useTranslation()
  const {
    notes,
    notesTags,
    notesSyncing,
    syncNotes,
    applyNoteRecord,
    removeNoteRecord,
    applyNoteTagRecord,
    removeNoteTagRecord,
    pickFiles,
    setNotesDraftDirty,
    setNotesDraftBusy,
    registerNotesDiscardHandler,
    setHeaderActions,
  } = useAppState()

  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [draft, setDraft] = useState<DraftState | null>(null)
  const [draftOrigin, setDraftOrigin] = useState<DraftState | null>(null)
  const [saving, setSaving] = useState(false)
  const [stagingAttachment, setStagingAttachment] = useState(false)
  const [addingDraftTag, setAddingDraftTag] = useState(false)
  const [search, setSearch] = useState('')
  const [tagFilter, setTagFilter] = useState<string | null>(null)
  const [newTagName, setNewTagName] = useState('')
  const [addingTag, setAddingTag] = useState(false)
  const [tagMutating, setTagMutating] = useState(false)
  const [attachments, setAttachments] = useState<NoteAttachmentRecord[]>([])
  const [conflictNote, setConflictNote] = useState<NoteRecord | null>(null)
  const [conflictDraft, setConflictDraft] = useState<DraftState | null>(null)
  const [renameTag, setRenameTag] = useState<{ id: string; name: string; originalName: string } | null>(null)
  const [deleteTag, setDeleteTag] = useState<{ id: string; name: string } | null>(null)
  const [deleteNote, setDeleteNote] = useState<NoteRecord | null>(null)
  const [discardOpen, setDiscardOpen] = useState(false)
  const [tagManagerOpen, setTagManagerOpen] = useState(false)
  const [attachmentManagerOpen, setAttachmentManagerOpen] = useState(false)
  const saveQueue = useRef<Promise<void>>(Promise.resolve())
  const editSession = useRef(0)
  const stagedAttachmentIds = useRef(new Set<string>())
  const pendingNavigation = useRef<(() => void) | null>(null)
  const richEditorRef = useRef<RichEditorHandle>(null)

  const selected = useMemo(
    () => notes.find((note) => note.id === selectedId) ?? null,
    [notes, selectedId],
  )
  const selectedTag = useMemo(
    () => notesTags.find((tag) => tag.id === tagFilter) ?? null,
    [notesTags, tagFilter],
  )
  const draftDirty = useMemo(() => !sameDraft(draft, draftOrigin), [draft, draftOrigin])
  const editorBusy = saving || stagingAttachment || addingDraftTag
  const conflictedNotes = useMemo(
    () => notes.filter((note) => note.syncState === 'conflict' || note.syncState === 'conflictDelete'),
    [notes],
  )

  const refreshAttachments = useCallback(() => {
    void notesAttachmentsList()
      .then(setAttachments)
      .catch(() => {})
  }, [])

  useEffect(() => {
    refreshAttachments()
  }, [refreshAttachments, notes])

  useEffect(() => {
    if (tagFilter && !notesTags.some((tag) => tag.id === tagFilter)) {
      setTagFilter(null)
    }
  }, [notesTags, tagFilter])

  useEffect(() => {
    if (selectedId && !notes.some((note) => note.id === selectedId) && !draftDirty && !saving) {
      setSelectedId(null)
      setDraft(null)
      setDraftOrigin(null)
    }
  }, [draftDirty, notes, saving, selectedId])

  useEffect(() => {
    if (!selected || !draft || draft.id !== selected.id || draftDirty || saving) return
    const nextDraft = draftFromNote(selected)
    if (!sameDraft(draftOrigin, nextDraft)) {
      setDraft(nextDraft)
      setDraftOrigin(nextDraft)
    }
  }, [draft, draftDirty, draftOrigin, saving, selected])

  useEffect(() => {
    const handleBeforeUnload = (event: BeforeUnloadEvent) => {
      if (draftDirty || editorBusy) event.preventDefault()
    }
    window.addEventListener('beforeunload', handleBeforeUnload)
    return () => window.removeEventListener('beforeunload', handleBeforeUnload)
  }, [draftDirty, editorBusy])

  const filteredNotes = useMemo(() => {
    const query = search.trim().toLowerCase()
    return notes.filter((note) => {
      if (tagFilter && !note.tagIds.includes(tagFilter)) {
        return false
      }
      if (!query) {
        return true
      }
      return (
        note.title.toLowerCase().includes(query) ||
        note.markdown.toLowerCase().includes(query)
      )
    })
  }, [notes, search, tagFilter])

  const tagName = useCallback(
    (id: string) => notesTags.find((tag) => tag.id === id)?.name ?? id.slice(0, 8),
    [notesTags],
  )

  const leaveEditor = useCallback((action: () => void) => {
    if (editorBusy) return
    if (draftDirty) {
      pendingNavigation.current = action
      setDiscardOpen(true)
      return
    }
    action()
  }, [draftDirty, editorBusy])

  const selectNote = useCallback((note: NoteRecord) => {
    if (draft?.id === note.id) return
    leaveEditor(() => {
      editSession.current += 1
      const nextDraft = draftFromNote(note)
      setSelectedId(note.id)
      setDraft(nextDraft)
      setDraftOrigin(nextDraft)
    })
  }, [draft?.id, leaveEditor])

  const beginNewNote = useCallback(async () => {
    const session = editSession.current + 1
    editSession.current = session
    try {
      const id = await notesNewId()
      if (editSession.current !== session) {
        return
      }
      setSelectedId(null)
      const nextDraft = {
        id,
        title: '',
        markdown: '',
        tagIds: tagFilter ? [tagFilter] : [],
        attachmentIds: [],
      }
      setDraft(nextDraft)
      setDraftOrigin(nextDraft)
    } catch (error) {
      toast.error(String(error))
    }
  }, [tagFilter])

  const startNewNote = useCallback(() => {
    leaveEditor(() => { void beginNewNote() })
  }, [beginNewNote, leaveEditor])

  const closeDraft = useCallback(() => {
    leaveEditor(() => {
      editSession.current += 1
      setSelectedId(null)
      setDraft(null)
      setDraftOrigin(null)
    })
  }, [leaveEditor])

  const saveDraft = useCallback((snapshot: DraftState): Promise<void> => {
    const session = editSession.current
    setSaving(true)
    const operation = saveQueue.current.then(async () => {
      const saved = await notesUpsert({
        id: snapshot.id ?? undefined,
        title: snapshot.title,
        markdown: snapshot.markdown,
        tagIds: snapshot.tagIds,
        attachmentIds: snapshot.attachmentIds,
      })
      if (editSession.current !== session) {
        return
      }
      applyNoteRecord(saved)
      if (editSession.current !== session) {
        return
      }
      setSelectedId(saved.id)
      stagedAttachmentIds.current.clear()
      const nextDraft = draftFromNote(saved)
      setDraft(nextDraft)
      setDraftOrigin(nextDraft)
      toast.success(t('notes.saved'))
    })
    saveQueue.current = operation.catch((error) => {
      if (editSession.current === session) {
        toast.error(String(error))
      }
    }).finally(() => {
      setSaving(false)
    })
    return saveQueue.current
  }, [applyNoteRecord, t])

  const cleanupStagedAttachments = useCallback(async (): Promise<boolean> => {
    const ids = [...stagedAttachmentIds.current]
    const results = await Promise.allSettled(ids.map((id) => notesAttachmentsDelete(id)))
    stagedAttachmentIds.current.clear()
    results.forEach((result, index) => {
      if (result.status === 'rejected') stagedAttachmentIds.current.add(ids[index])
    })
    const failure = results.find((result): result is PromiseRejectedResult => result.status === 'rejected')
    if (failure) {
      toast.error(String(failure.reason))
      refreshAttachments()
      return false
    }
    if (ids.length > 0) refreshAttachments()
    return true
  }, [refreshAttachments])

  const confirmDelete = useCallback(
    async (note: NoteRecord) => {
      try {
        await notesDelete(note.id)
        removeNoteRecord(note.id)
        if (selectedId === note.id) {
          await cleanupStagedAttachments()
          editSession.current += 1
          setSelectedId(null)
          setDraft(null)
          setDraftOrigin(null)
        }
        setDeleteNote(null)
      } catch (error) {
        toast.error(String(error))
      }
    },
    [cleanupStagedAttachments, removeNoteRecord, selectedId],
  )

  const addTagToDraft = useCallback(
    async (name: string) => {
      const trimmed = name.trim()
      if (!trimmed || !draft || editorBusy) {
        return
      }
      setAddingDraftTag(true)
      try {
        const tag = await notesTagsCreate(trimmed)
        applyNoteTagRecord(tag)
        setDraft((current) =>
          current && !current.tagIds.includes(tag.id)
            ? { ...current, tagIds: [...current.tagIds, tag.id] }
            : current,
        )
      } catch (error) {
        toast.error(String(error))
      } finally {
        setAddingDraftTag(false)
      }
    },
    [applyNoteTagRecord, draft, editorBusy],
  )

  const createSidebarTag = useCallback(async () => {
    const name = newTagName.trim()
    if (!name || tagMutating) return
    setTagMutating(true)
    try {
      const tag = await notesTagsCreate(name)
      applyNoteTagRecord(tag)
      setNewTagName('')
      setAddingTag(false)
    } catch (error) {
      toast.error(String(error))
    } finally {
      setTagMutating(false)
    }
  }, [applyNoteTagRecord, newTagName, tagMutating])

  const renameSidebarTag = useCallback(async () => {
    if (!renameTag || tagMutating) return
    const name = renameTag.name.trim()
    if (!name || name === renameTag.originalName) return
    setTagMutating(true)
    try {
      const tag = await notesTagsRename(renameTag.id, name)
      applyNoteTagRecord(tag)
      setRenameTag(null)
    } catch (error) {
      toast.error(String(error))
    } finally {
      setTagMutating(false)
    }
  }, [applyNoteTagRecord, renameTag, tagMutating])

  const deleteSidebarTag = useCallback(async () => {
    if (!deleteTag || tagMutating) return
    setTagMutating(true)
    try {
      await notesTagsDelete(deleteTag.id)
      removeNoteTagRecord(deleteTag.id)
      if (tagFilter === deleteTag.id) setTagFilter(null)
      setDeleteTag(null)
    } catch (error) {
      toast.error(String(error))
    } finally {
      setTagMutating(false)
    }
  }, [deleteTag, removeNoteTagRecord, tagFilter, tagMutating])

  const stageAttachment = useCallback(
    async (kind: 'image' | 'file', insertIntoNote = true) => {
      if (!draft || editorBusy) {
        return
      }
      setStagingAttachment(true)
      try {
        const paths = await pickFiles(false)
        const path = paths[0]
        if (!path) {
          return
        }
        const record = await notesAttachmentsStage(path, kind)
        stagedAttachmentIds.current.add(record.id)
        setDraft((current) =>
          current
            ? {
                ...current,
                attachmentIds: [...current.attachmentIds, record.id],
              }
            : current,
        )
        if (insertIntoNote) {
          richEditorRef.current?.insertAttachment(record, kind)
        }
        refreshAttachments()
      } catch (error) {
        toast.error(String(error))
      } finally {
        setStagingAttachment(false)
      }
    },
    [draft, editorBusy, pickFiles, refreshAttachments],
  )

  const removeAttachment = useCallback(
    async (attachmentId: string) => {
      if (!draft) {
        return
      }

      const originalDraft = draft
      const nextDraft = {
        ...draft,
        attachmentIds: draft.attachmentIds.filter((id) => id !== attachmentId),
        markdown: draft.markdown.replace(
          new RegExp(`!\\[[^\\]]*\\]\\(colink-attachment://${attachmentId}\\)|\\[[^\\]]*\\]\\(colink-attachment://${attachmentId}\\)`, 'g'),
          '',
        ),
      }
      const staged = stagedAttachmentIds.current.has(attachmentId)
      const session = editSession.current
      let cloudReferenceRemoved = false

      const persistDraft = async (snapshot: DraftState) => {
        const saved = await notesUpsert({
          id: snapshot.id ?? undefined,
          title: snapshot.title,
          markdown: snapshot.markdown,
          tagIds: snapshot.tagIds,
          attachmentIds: snapshot.attachmentIds,
        })
        applyNoteRecord(saved)
        if (editSession.current === session) {
          const savedDraft = draftFromNote(saved)
          setSelectedId(saved.id)
          setDraft(savedDraft)
          setDraftOrigin(savedDraft)
        }
      }

      setStagingAttachment(true)
      try {
        if (!staged) {
          await persistDraft(nextDraft)
          cloudReferenceRemoved = true
          const syncOutcome = await syncNotes()
          if (!syncOutcome || !['ok', 'unsupported'].includes(syncOutcome.status)) {
            await persistDraft(originalDraft)
            if (syncOutcome?.status === 'offline') {
              toast.info(t('notes.offlineHint'))
            } else if (syncOutcome?.status === 'error') {
              toast.error(syncOutcome.message ?? t('notes.syncFailed'))
            } else if (!syncOutcome) {
              toast.error(t('notes.syncFailed'))
            }
            return
          }
        }

        const outcome = await notesAttachmentsDelete(attachmentId)
        if (staged) {
          stagedAttachmentIds.current.delete(attachmentId)
          setDraft(nextDraft)
        }
        if (outcome.unsupported) {
          toast.info(t('notes.serverUnsupported'), { id: 'notes-server-unsupported' })
        }
        refreshAttachments()
      } catch (error) {
        const stillReferenced = hasProtocolCode(error, 6006)
        if (cloudReferenceRemoved && !stillReferenced) {
          try {
            await persistDraft(originalDraft)
          } catch (restoreError) {
            console.error('Failed to restore note attachment reference', restoreError)
          }
        }
        if (stillReferenced) {
          toast.error(t('notes.attachmentStillReferenced'))
        } else {
          toast.error(readErrorMessage(error))
        }
        return
      } finally {
        setStagingAttachment(false)
      }
    },
    [applyNoteRecord, draft, refreshAttachments, syncNotes, t],
  )

  useEffect(() => () => {
    editSession.current += 1
  }, [])

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!draft) return
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
        event.preventDefault()
        if (draftDirty && !editorBusy) void saveDraft(draft)
      } else if (event.key === 'Escape') {
        if (tagManagerOpen) {
          setTagManagerOpen(false)
        } else if (attachmentManagerOpen) {
          setAttachmentManagerOpen(false)
        }
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [attachmentManagerOpen, draft, draftDirty, editorBusy, saveDraft, tagManagerOpen])

  const discardAndContinue = useCallback(async () => {
    if (!await cleanupStagedAttachments()) return
    setDiscardOpen(false)
    setDraft(null)
    setDraftOrigin(null)
    const action = pendingNavigation.current
    pendingNavigation.current = null
    action?.()
  }, [cleanupStagedAttachments])

  const discardForNavigation = useCallback(async () => {
    if (editorBusy || !await cleanupStagedAttachments()) return false
    editSession.current += 1
    setSelectedId(null)
    setDraft(null)
    setDraftOrigin(null)
    pendingNavigation.current = null
    return true
  }, [cleanupStagedAttachments, editorBusy])

  useEffect(() => {
    setNotesDraftDirty(draftDirty)
    return () => setNotesDraftDirty(false)
  }, [draftDirty, setNotesDraftDirty])

  useEffect(() => {
    setNotesDraftBusy(editorBusy)
    return () => setNotesDraftBusy(false)
  }, [editorBusy, setNotesDraftBusy])

  useEffect(() => {
    registerNotesDiscardHandler(discardForNavigation)
    return () => registerNotesDiscardHandler(null)
  }, [discardForNavigation, registerNotesDiscardHandler])

  const runSync = useCallback(async () => {
    const outcome = await syncNotes()
    if (outcome) {
      if (outcome.repairedReferences > 0) {
        toast.warning(t('notes.referencesRemovedToast', { count: outcome.repairedReferences }))
      }
      if (outcome.status === 'offline') {
        toast.info(outcome.message ?? t('notes.offlineHint'))
      } else if (outcome.status === 'unsupported') {
        toast.info(t('notes.serverUnsupported'), { id: 'notes-server-unsupported' })
      } else if (outcome.status === 'storage_full') {
        toast.error(t('notes.storageFull'), { id: 'notes-storage-full' })
      } else if (outcome.status === 'error') {
        toast.error(outcome.message ?? t('notes.syncFailed'))
      } else if (outcome.conflicts > 0) {
        toast.warning(t('notes.conflictDetectedToast', { count: outcome.conflicts }))
      } else {
        toast.success(t('notes.syncedToast'))
      }
    }
  }, [syncNotes, t])

  useEffect(() => {
    setHeaderActions(
      <>
        <Button variant="secondary" size="sm" onClick={() => void runSync()} disabled={notesSyncing}>
          <RefreshCw className={cn('h-3.5 w-3.5', notesSyncing && 'animate-spin')} />
          {notesSyncing ? t('notes.syncing') : t('notes.syncNow')}
        </Button>
        <Button size="sm" onClick={startNewNote} disabled={editorBusy}>
          <Plus className="h-3.5 w-3.5" />
          {t('notes.newNote')}
        </Button>
      </>,
    )
    return () => setHeaderActions(null)
  }, [editorBusy, notesSyncing, runSync, setHeaderActions, startNewNote, t])

  const resolveConflict = useCallback(
    async (resolution: 'local' | 'cloud' | 'confirm_delete' | 'cancel_delete') => {
      if (!conflictNote) {
        return
      }
      try {
        await notesResolveConflict({
          noteId: conflictNote.id,
          resolution,
        })
        setConflictNote(null)
      } catch (error) {
        toast.error(String(error))
      }
    },
    [conflictNote],
  )

  const resolveMerged = useCallback(async () => {
    if (!conflictNote) {
      return
    }
    try {
      await notesResolveConflict({
        noteId: conflictNote.id,
        resolution: 'merged',
        title: conflictDraft?.title,
        markdown: conflictDraft?.markdown,
        tagIds: conflictDraft?.tagIds,
        attachmentIds: conflictDraft?.attachmentIds,
      })
      setConflictNote(null)
    } catch (error) {
      toast.error(String(error))
    }
  }, [conflictNote, conflictDraft])

  // Do not interrupt an active local edit with conflict resolution.
  useEffect(() => {
    if (!draftDirty && !editorBusy && conflictedNotes.length > 0 && (!conflictNote || !conflictedNotes.includes(conflictNote))) {
      setConflictNote(conflictedNotes[0])
      setConflictDraft(conflictDraftFromNote(conflictedNotes[0]))
    }
  }, [conflictedNotes, conflictNote, draftDirty, editorBusy])

  const formatSize = useCallback((bytes: number) => {
    if (bytes >= 1024 * 1024) {
      return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
    }
    if (bytes >= 1024) {
      return `${(bytes / 1024).toFixed(1)} KB`
    }
    return `${bytes} B`
  }, [])

  const stateBadge = useCallback(
    (note: NoteRecord) => {
      if (note.syncState === 'synced') return null
      const map: Record<string, { label: string; className: string }> = {
        pending: { label: t('notes.pending'), className: 'bg-amber-500/15 text-amber-600 dark:text-amber-400' },
        pendingDelete: { label: t('notes.pendingDelete'), className: 'bg-red-500/15 text-red-600 dark:text-red-400' },
        conflict: { label: t('notes.conflict'), className: 'bg-red-500/15 text-red-600 dark:text-red-400' },
        conflictDelete: { label: t('notes.conflict'), className: 'bg-red-500/15 text-red-600 dark:text-red-400' },
      }
      const badge = map[note.syncState]
      if (!badge) {
        return null
      }
      return (
        <span className={cn('shrink-0 whitespace-nowrap rounded-full px-2 py-0.5 text-[11px] font-medium', badge.className)}>
          {badge.label}
        </span>
      )
    },
    [t],
  )

  const draftAttachments = useMemo(() => {
    if (!draft) {
      return []
    }
    return draft.attachmentIds
      .map((id) => attachments.find((record) => record.id === id))
      .filter((record): record is NoteAttachmentRecord => Boolean(record))
  }, [draft, attachments])

  return (
    <div className="flex h-full min-h-0 overflow-hidden bg-[hsl(var(--bg))]">
      {/* List pane */}
      <aside className={cn(
        'w-full min-w-0 flex-col border-r border-[hsl(var(--border))] bg-[hsl(var(--panel))] md:flex md:w-80 md:min-w-72 lg:w-[22rem]',
        draft ? 'hidden md:flex' : 'flex',
      )}>
        <div className="space-y-3 border-b border-[hsl(var(--border))] p-4">
          <div className="relative min-w-0">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-[hsl(var(--muted))]" />
            <Input
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder={t('notes.searchPlaceholder')}
              className="h-9 w-full rounded-full bg-[hsl(var(--panel-2))] pl-8 shadow-none"
            />
          </div>

          <div className="flex items-start justify-between gap-2">
            <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
              <button
                type="button"
                onClick={() => setTagFilter(null)}
                className={cn(
                  'inline-flex shrink-0 items-center gap-1 whitespace-nowrap rounded-full border px-2 py-0.5 text-[11px] transition-colors',
                  tagFilter === null
                    ? 'border-transparent bg-[hsl(var(--text))] text-[hsl(var(--panel))]'
                    : 'border-[hsl(var(--border))] text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))]',
                )}
              >
                <TagIcon className="h-3 w-3" />
                {t('notes.allTags')}
              </button>
              {notesTags.map((tag) => (
                <button
                  key={tag.id}
                  type="button"
                  onClick={() => setTagFilter(tagFilter === tag.id ? null : tag.id)}
                  className={cn(
                    'inline-flex shrink-0 items-center gap-1 whitespace-nowrap rounded-full border px-2 py-0.5 text-[11px] transition-colors',
                    tagFilter === tag.id
                      ? 'border-transparent bg-[hsl(var(--text))] text-[hsl(var(--panel))]'
                      : 'border-[hsl(var(--border))] text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))]',
                  )}
                >
                  <TagIcon className="h-3 w-3" />
                  {tag.name}
                </button>
              ))}
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <button
                type="button"
                title={t('notes.newTag')}
                aria-label={t('notes.newTag')}
                disabled={tagMutating}
                onClick={() => setAddingTag(true)}
                className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-[hsl(var(--border))] text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))] disabled:pointer-events-none disabled:opacity-35"
              >
                {tagMutating ? <RefreshCw className="h-3 w-3 animate-spin" /> : <Plus className="h-3 w-3" />}
              </button>
              <button
                type="button"
                title={t('notes.renameTag')}
                aria-label={t('notes.renameTag')}
                disabled={!selectedTag || tagMutating}
                className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-[hsl(var(--border))] text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))] disabled:pointer-events-none disabled:opacity-35"
                onClick={() => selectedTag && setRenameTag({
                  id: selectedTag.id,
                  name: selectedTag.name,
                  originalName: selectedTag.name,
                })}
              >
                <Pencil className="h-3 w-3" />
              </button>
              <button
                type="button"
                title={t('notes.deleteTag')}
                aria-label={t('notes.deleteTag')}
                disabled={!selectedTag || tagMutating}
                className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-[hsl(var(--border))] text-[hsl(var(--muted))] transition-colors hover:border-red-500/30 hover:bg-red-500/10 hover:text-red-500 disabled:pointer-events-none disabled:opacity-35"
                onClick={() => selectedTag && setDeleteTag({ id: selectedTag.id, name: selectedTag.name })}
              >
                <Trash2 className="h-3 w-3" />
              </button>
            </div>
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto">
          {filteredNotes.length === 0 ? (
            <div className="flex flex-col items-center gap-2 px-6 py-12 text-center text-[hsl(var(--muted))]">
              <FileText className="h-8 w-8" />
              <p className="text-sm">{search || tagFilter ? t('notes.noResults') : t('notes.empty')}</p>
              <p className="text-xs">{search || tagFilter ? t('notes.noResultsHint') : t('notes.emptyHint')}</p>
            </div>
          ) : (
            <ul className="space-y-1.5 p-2">
              {filteredNotes.map((note) => {
                const preview = note.markdown
                  .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
                  .replace(/[#>*`_~\-]/g, ' ')
                  .replace(/\s+/g, ' ')
                  .trim()

                return (
                  <li key={note.id}>
                    <button
                      type="button"
                      onClick={() => selectNote(note)}
                      className={cn(
                        'w-full rounded-lg border border-transparent px-3 py-3 text-left transition-colors hover:bg-[hsl(var(--panel-2)/0.6)]',
                        selectedId === note.id && 'border-[hsl(var(--accent)/0.35)] bg-[hsl(var(--accent)/0.08)]',
                      )}
                    >
                      <div className="flex min-w-0 items-center justify-between gap-2">
                        <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-[hsl(var(--text))]">
                          {note.title || t('notes.untitled')}
                        </span>
                        {stateBadge(note)}
                      </div>
                      {preview ? (
                        <p className="mt-1 truncate text-xs text-[hsl(var(--muted))]">
                          {preview}
                        </p>
                      ) : null}
                      <div className="mt-2 flex flex-wrap items-center gap-1.5 text-[11px] text-[hsl(var(--muted))]">
                        <span>{formatTimestamp(note.updatedAt)}</span>
                        {note.tagIds.map((id) => (
                          <span
                            key={id}
                            className="inline-flex min-w-0 max-w-full shrink-0 items-center gap-1 rounded-full bg-[hsl(var(--panel-2))] px-1.5 py-0.5 text-[10px]"
                          >
                            <TagIcon className="h-2.5 w-2.5 shrink-0" />
                            <span className="truncate">{tagName(id)}</span>
                          </span>
                        ))}
                      </div>
                    </button>
                  </li>
                )
              })}
            </ul>
          )}
        </div>

      </aside>

      {/* Editor pane */}
      <section className={cn(
        'min-w-0 flex-1 flex-col overflow-y-auto bg-[hsl(var(--bg))] md:flex',
        draft ? 'flex' : 'hidden',
      )}>
        {draft ? (
          <div className="flex h-full min-h-0 flex-col">
            <div className="sticky top-0 z-10 flex h-20 shrink-0 items-center gap-2 overflow-hidden border-b border-[hsl(var(--border))] bg-[hsl(var(--panel))] px-3 sm:px-5">
              <Button
                variant="ghost"
                size="sm"
                className="px-2 md:hidden"
                disabled={editorBusy}
                onClick={closeDraft}
                title={t('notes.allNotes')}
              >
                <ArrowLeft className="h-4 w-4" />
              </Button>
              <div className="min-w-0 flex-1">
                <Input
                  key={draft.id}
                  autoFocus={selectedId === null}
                  value={draft.title}
                  disabled={editorBusy}
                  onChange={(event) => {
                    setDraft({ ...draft, title: event.target.value })
                  }}
                  placeholder={t('notes.titlePlaceholder')}
                  className="w-full border-none bg-transparent text-lg font-semibold shadow-none focus-visible:ring-0 sm:text-xl"
                />
                {selected && (
                  <div className="flex h-5 min-w-0 items-center gap-2 overflow-hidden px-3 text-xs text-[hsl(var(--muted))]">
                    <span className="min-w-0 truncate whitespace-nowrap">
                      {t('notes.lastUpdated', { time: formatTimestamp(selected.updatedAt) })}
                    </span>
                    {stateBadge(selected)}
                  </div>
                )}
              </div>
            </div>

            <RichEditor
              key={draft.id}
              ref={richEditorRef}
              markdown={draft.markdown}
              disabled={editorBusy}
              onChange={(markdown) => setDraft((current) => current ? { ...current, markdown } : current)}
              onAddImage={() => void stageAttachment('image')}
              onAddAttachment={() => void stageAttachment('file')}
              onOpenAttachment={(id) => {
                void notesAttachmentsOpen(id)
                  .catch((error) => {
                    if (hasHttpStatus(error, 404)) {
                      toast.info(t('notes.serverUnsupported'), { id: 'notes-server-unsupported' })
                    } else {
                      toast.error(t('notes.attachmentUnavailable'))
                    }
                  })
              }}
              toolbarRight={
                <>
                  <button
                    type="button"
                    title={t('notes.tags')}
                    aria-label={t('notes.tags')}
                    disabled={editorBusy}
                    onClick={() => {
                      setAttachmentManagerOpen(false)
                      setTagManagerOpen(true)
                    }}
                    className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))] disabled:pointer-events-none disabled:opacity-35"
                  >
                    <TagIcon className="h-4 w-4" />
                  </button>
                  <button
                    type="button"
                    title={t('notes.attachments')}
                    aria-label={t('notes.attachments')}
                    disabled={editorBusy}
                    onClick={() => {
                      setTagManagerOpen(false)
                      setAttachmentManagerOpen(true)
                    }}
                    className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))] disabled:pointer-events-none disabled:opacity-35"
                  >
                    <Paperclip className="h-4 w-4" />
                  </button>
                  <button
                    type="button"
                    title={t('notes.deleteNote')}
                    aria-label={t('notes.deleteNote')}
                    disabled={!selected || editorBusy}
                    onClick={() => selected && setDeleteNote(selected)}
                    className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-[hsl(var(--muted))] transition-colors hover:bg-red-500/10 hover:text-red-500 disabled:pointer-events-none disabled:opacity-35"
                  >
                    <Trash2 className="h-4 w-4" />
                  </button>
                  <button
                    type="button"
                    title={`${saving ? t('notes.saving') : t('common.save')} (Ctrl+S)`}
                    aria-label={saving ? t('notes.saving') : t('common.save')}
                    disabled={!draftDirty || editorBusy}
                    onClick={() => {
                      void saveDraft(draft)
                    }}
                    className={cn(
                      'inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md transition-colors disabled:pointer-events-none disabled:opacity-35',
                      draftDirty
                        ? 'bg-[hsl(var(--text))] text-[hsl(var(--panel))] hover:opacity-85'
                        : 'text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]',
                    )}
                  >
                    {saving ? <RefreshCw className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
                  </button>
                </>
              }
            />
          </div>
        ) : (
          <div className="flex h-full flex-col items-center justify-center gap-2 text-[hsl(var(--muted))]">
            <FileText className="h-10 w-10" />
            <p className="text-sm">{t('notes.noNoteSelected')}</p>
            <Button variant="secondary" size="sm" onClick={startNewNote}>
              <Plus className="h-3.5 w-3.5" /> {t('notes.newNote')}
            </Button>
          </div>
        )}
      </section>

      {tagManagerOpen && draft && (
        <NotesDialog
          title={t('notes.tags')}
          width="md"
          onClose={() => setTagManagerOpen(false)}
        >
          <div className="flex max-h-64 flex-wrap gap-2 overflow-y-auto">
            {notesTags.map((tag) => {
              const selected = draft.tagIds.includes(tag.id)
              return (
                <button
                  key={tag.id}
                  type="button"
                  aria-pressed={selected}
                  disabled={editorBusy}
                  onClick={() => setDraft({
                    ...draft,
                    tagIds: selected ? draft.tagIds.filter((id) => id !== tag.id) : [...draft.tagIds, tag.id],
                  })}
                  className={cn(
                    'inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-full border px-2.5 py-1 text-xs transition-colors disabled:opacity-40',
                    selected
                      ? 'border-[hsl(var(--accent)/0.4)] bg-[hsl(var(--accent)/0.12)] text-[hsl(var(--text))]'
                      : 'border-[hsl(var(--border))] text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))]',
                  )}
                >
                  <TagIcon className="h-3 w-3" />
                  {tag.name}
                  {selected && <X className="h-3 w-3" />}
                </button>
              )
            })}
          </div>
          <div className="mt-4">
            <Input
              disabled={editorBusy}
              placeholder={t('notes.addTag')}
              onKeyDown={(event) => {
                const input = event.currentTarget
                if (event.key === 'Enter' && input.value.trim()) {
                  void addTagToDraft(input.value)
                  input.value = ''
                }
              }}
            />
          </div>
        </NotesDialog>
      )}

      {attachmentManagerOpen && draft && (
        <NotesDialog
          title={t('notes.attachments')}
          width="lg"
          onClose={() => setAttachmentManagerOpen(false)}
        >
          {draftAttachments.length > 0 ? (
            <div className="max-h-72 space-y-2 overflow-y-auto pr-1">
              {draftAttachments.map((record) => {
                const isImage = record.kind === 'image' || record.mediaType?.startsWith('image/')
                return (
                  <div
                    key={record.id}
                    className="flex min-w-0 items-center gap-3 rounded-lg border border-[hsl(var(--border))] px-3 py-2.5"
                  >
                    {isImage ? (
                      <Image className="h-4 w-4 shrink-0 text-[hsl(var(--accent))]" />
                    ) : (
                      <FileText className="h-4 w-4 shrink-0 text-[hsl(var(--accent))]" />
                    )}
                    <span className="min-w-0 flex-1 truncate text-sm text-[hsl(var(--text))]">{record.fileName}</span>
                    <span className="shrink-0 text-[11px] text-[hsl(var(--muted))]">{formatSize(record.size)}</span>
                    <button
                      type="button"
                      disabled={editorBusy}
                      title={t('notes.insertIntoNote')}
                      aria-label={t('notes.insertIntoNote')}
                      onClick={() => {
                        setAttachmentManagerOpen(false)
                        richEditorRef.current?.insertAttachment(record, isImage ? 'image' : 'file')
                      }}
                      className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))] disabled:opacity-40"
                    >
                      <Plus className="h-3.5 w-3.5" />
                    </button>
                    <button
                      type="button"
                      disabled={editorBusy}
                      title={t('notes.removeAttachment')}
                      aria-label={t('notes.removeAttachment')}
                      onClick={() => void removeAttachment(record.id)}
                      className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-[hsl(var(--muted))] transition-colors hover:bg-red-500/10 hover:text-red-500 disabled:opacity-40"
                    >
                      <X className="h-3.5 w-3.5" />
                    </button>
                  </div>
                )
              })}
            </div>
          ) : (
            <div className="flex h-28 flex-col items-center justify-center gap-2 text-[hsl(var(--muted))]">
              <Paperclip className="h-5 w-5" />
              <span className="text-sm">{t('notes.noAttachments')}</span>
            </div>
          )}
          <div className="mt-4 flex justify-end">
            <Button
              variant="secondary"
              size="sm"
              disabled={editorBusy}
              onClick={() => void stageAttachment('file', false)}
            >
              {stagingAttachment ? <RefreshCw className="h-3.5 w-3.5 animate-spin" /> : <Paperclip className="h-3.5 w-3.5" />}
              {t('notes.addAttachment')}
            </Button>
          </div>
        </NotesDialog>
      )}

      {addingTag && (
        <NotesDialog
          title={t('notes.newTag')}
          confirmLabel={t('notes.newTag')}
          confirmDisabled={!newTagName.trim() || tagMutating}
          onClose={() => {
            if (tagMutating) return
            setNewTagName('')
            setAddingTag(false)
          }}
          onConfirm={() => { void createSidebarTag() }}
        >
          <Input
            autoFocus
            value={newTagName}
            disabled={tagMutating}
            onChange={(event) => setNewTagName(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && newTagName.trim() && !tagMutating) {
                void createSidebarTag()
              } else if (event.key === 'Escape' && !tagMutating) {
                setNewTagName('')
                setAddingTag(false)
              }
            }}
            placeholder={t('notes.newTag')}
          />
        </NotesDialog>
      )}

      {renameTag && (
        <NotesDialog
          title={t('notes.renameTag')}
          confirmLabel={t('common.save')}
          confirmDisabled={tagMutating || !renameTag.name.trim() || renameTag.name.trim() === renameTag.originalName}
          onClose={() => { if (!tagMutating) setRenameTag(null) }}
          onConfirm={() => { void renameSidebarTag() }}
        >
          <Input
            autoFocus
            value={renameTag.name}
            disabled={tagMutating}
            onChange={(event) => setRenameTag({ ...renameTag, name: event.target.value })}
            onKeyDown={(event) => {
              const nextName = renameTag.name.trim()
              if (event.key === 'Enter' && nextName && nextName !== renameTag.originalName && !tagMutating) {
                void renameSidebarTag()
              } else if (event.key === 'Escape' && !tagMutating) {
                setRenameTag(null)
              }
            }}
            placeholder={t('notes.renameTagPrompt')}
          />
        </NotesDialog>
      )}

      {deleteTag && (
        <NotesDialog
          title={t('notes.deleteTag')}
          description={t('notes.deleteTagConfirm', { name: deleteTag.name })}
          confirmLabel={t('notes.deleteTag')}
          danger
          confirmDisabled={tagMutating}
          onClose={() => { if (!tagMutating) setDeleteTag(null) }}
          onConfirm={() => { void deleteSidebarTag() }}
        />
      )}

      {deleteNote && (
        <NotesDialog
          title={t('notes.deleteNote')}
          description={t('notes.deleteConfirmDescription', { title: deleteNote.title || t('notes.untitled') })}
          confirmLabel={t('notes.deleteNote')}
          danger
          onClose={() => setDeleteNote(null)}
          onConfirm={() => { void confirmDelete(deleteNote) }}
        />
      )}

      {discardOpen && (
        <NotesDialog
          title={t('notes.discardChangesTitle')}
          description={t('notes.discardChangesDescription')}
          confirmLabel={t('notes.discardChanges')}
          danger
          onClose={() => {
            pendingNavigation.current = null
            setDiscardOpen(false)
          }}
          onConfirm={() => { void discardAndContinue() }}
        />
      )}

      {/* Conflict resolution dialog */}
      {conflictNote && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-6">
          <div className="max-h-[90vh] w-full max-w-3xl overflow-y-auto rounded-lg border border-[hsl(var(--border))] bg-[hsl(var(--panel))] p-5 shadow-xl">
            <div className="mb-3 flex items-center gap-2">
              <TriangleAlert className="h-5 w-5 text-amber-500" />
              <h2 className="text-base font-semibold text-[hsl(var(--text))]">
                {conflictNote.conflictKind === 'cloudDeleted'
                  ? t('notes.cloudDeletedTitle')
                  : conflictNote.conflictKind === 'delete'
                    ? t('notes.deleteConflictTitle')
                    : t('notes.conflictTitle')}
              </h2>
            </div>
            <p className="mb-4 text-sm text-[hsl(var(--muted))]">
              {conflictNote.conflictKind === 'cloudDeleted'
                ? t('notes.cloudDeletedDescription', { title: conflictNote.title || t('notes.untitled') })
                : conflictNote.conflictKind === 'delete'
                  ? t('notes.deleteConflictDescription')
                  : t('notes.conflictDescription')}
            </p>

            {conflictNote.conflictKind === 'edit' && (
              <div className="mb-4 grid grid-cols-1 gap-3 sm:grid-cols-2">
                <div className="rounded-lg border border-[hsl(var(--border))] p-3">
                  <p className="mb-1 text-xs font-medium text-[hsl(var(--muted))]">{t('notes.localVersion')}</p>
                  <p className="text-sm font-medium text-[hsl(var(--text))]">{conflictNote.title || t('notes.untitled')}</p>
                  <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap text-xs text-[hsl(var(--muted))]">{conflictNote.markdown}</pre>
                </div>
                <div className="rounded-lg border border-[hsl(var(--border))] p-3">
                  <p className="mb-1 text-xs font-medium text-[hsl(var(--muted))]">{t('notes.cloudVersion')}</p>
                  <p className="text-sm font-medium text-[hsl(var(--text))]">{conflictNote.conflictTitle || t('notes.untitled')}</p>
                  <pre className="mt-1 max-h-32 overflow-auto whitespace-pre-wrap text-xs text-[hsl(var(--muted))]">{conflictNote.conflictMarkdown}</pre>
                </div>
              </div>
            )}

            {conflictNote.conflictKind === 'edit' && conflictDraft && (
              <div className="mb-4 space-y-2">
                <Input
                  value={conflictDraft.title}
                  onChange={(event) => setConflictDraft({ ...conflictDraft, title: event.target.value })}
                  placeholder={t('notes.mergedResult')}
                />
                <textarea
                  value={conflictDraft.markdown}
                  onChange={(event) => setConflictDraft({ ...conflictDraft, markdown: event.target.value })}
                  className="h-48 w-full resize-y rounded border border-[hsl(var(--border))] bg-[hsl(var(--bg))] p-3 font-mono text-xs text-[hsl(var(--text))] outline-none focus:ring-2 focus:ring-[hsl(var(--accent))]"
                  placeholder={t('notes.markdownPlaceholder')}
                />
              </div>
            )}

            <div className="flex flex-wrap justify-end gap-2">
              {conflictNote.conflictKind === 'edit' && (
                <>
                  <Button variant="secondary" size="sm" onClick={() => void resolveConflict('cloud')}>
                    {t('notes.keepCloud')}
                  </Button>
                  <Button variant="secondary" size="sm" onClick={() => void resolveMerged()}>
                    {t('notes.mergeManually')}
                  </Button>
                  <Button size="sm" onClick={() => void resolveConflict('local')}>
                    {t('notes.keepLocal')}
                  </Button>
                </>
              )}
              {conflictNote.conflictKind === 'cloudDeleted' && (
                <>
                  <Button variant="danger" size="sm" onClick={() => void resolveConflict('cloud')}>
                    {t('notes.discardLocal')}
                  </Button>
                  <Button size="sm" onClick={() => void resolveConflict('local')}>
                    {t('notes.saveAsNew')}
                  </Button>
                </>
              )}
              {conflictNote.conflictKind === 'delete' && (
                <>
                  <Button variant="secondary" size="sm" onClick={() => void resolveConflict('cancel_delete')}>
                    {t('notes.keepCloudVersion')}
                  </Button>
                  <Button variant="danger" size="sm" onClick={() => void resolveConflict('confirm_delete')}>
                    {t('notes.deleteAnyway')}
                  </Button>
                </>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

export const notesDefaultSettings = defaultSettings
