import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useMemo,
  useState,
  type MouseEvent,
  type ReactNode,
} from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import Image from '@tiptap/extension-image'
import Link from '@tiptap/extension-link'
import Placeholder from '@tiptap/extension-placeholder'
import TaskItem from '@tiptap/extension-task-item'
import TaskList from '@tiptap/extension-task-list'
import {
  EditorContent,
  NodeViewWrapper,
  ReactNodeViewRenderer,
  useEditor,
  useEditorState,
  type Editor,
  type NodeViewProps,
} from '@tiptap/react'
import StarterKit from '@tiptap/starter-kit'
import {
  Bold,
  Code,
  FileCode2,
  Heading1,
  Heading2,
  Heading3,
  ImageIcon,
  Italic,
  List,
  ListChecks,
  ListOrdered,
  Paperclip,
  Quote,
  Redo2,
  Strikethrough,
  Undo2,
} from 'lucide-react'
import { Markdown } from 'tiptap-markdown'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'

import { notesAttachmentsResolvePath } from '../../lib/api'
import { hasHttpStatus } from '../../lib/command-error'
import { cn } from '../../lib/utils'

const ATTACHMENT_URI_PREFIX = 'colink-attachment://'
const ATTACHMENT_URI_PATTERN =
  /^colink-attachment:\/\/[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/

function getMarkdown(editor: { storage: unknown }): string {
  return (editor.storage as { markdown: { getMarkdown: () => string } }).markdown.getMarkdown()
}

export interface RichEditorHandle {
  insertAttachment: (attachment: { id: string; fileName: string }, kind: 'image' | 'file') => void
}

interface RichEditorProps {
  markdown: string
  disabled?: boolean
  onChange: (markdown: string) => void
  onAddImage: () => void
  onAddAttachment: () => void
  onOpenAttachment: (attachmentId: string) => void
  toolbarRight?: ReactNode
}

function getToolbarState(editor: Editor) {
  return {
    canUndo: editor.can().undo(),
    canRedo: editor.can().redo(),
    headings: ([1, 2, 3] as const).map((level) => editor.isActive('heading', { level })),
    bold: editor.isActive('bold'),
    italic: editor.isActive('italic'),
    strike: editor.isActive('strike'),
    inlineCode: editor.isActive('code'),
    bulletList: editor.isActive('bulletList'),
    orderedList: editor.isActive('orderedList'),
    taskList: editor.isActive('taskList'),
    blockquote: editor.isActive('blockquote'),
    codeBlock: editor.isActive('codeBlock'),
  }
}

function AttachmentImageView({ node, selected }: NodeViewProps) {
  const { t } = useTranslation()
  const src = String(node.attrs.src ?? '')
  const attachmentId = ATTACHMENT_URI_PATTERN.test(src)
    ? src.slice(ATTACHMENT_URI_PREFIX.length)
    : null
  const [resolvedSrc, setResolvedSrc] = useState<string | null>(null)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    let disposed = false
    setResolvedSrc(null)
    setFailed(false)
    if (!attachmentId) {
      setFailed(true)
      return () => { disposed = true }
    }
    void notesAttachmentsResolvePath(attachmentId)
      .then((path) => {
        if (!disposed) setResolvedSrc(convertFileSrc(path))
      })
      .catch((error) => {
        if (!disposed) {
          setFailed(true)
          if (hasHttpStatus(error, 404)) {
            toast.info(t('notes.serverUnsupported'), { id: 'notes-server-unsupported' })
          }
        }
      })
    return () => { disposed = true }
  }, [attachmentId, t])

  return (
    <NodeViewWrapper className="rich-editor-image" data-selected={selected || undefined}>
      {resolvedSrc ? (
        <img src={resolvedSrc} alt={String(node.attrs.alt ?? '')} draggable={false} />
      ) : failed ? (
        <div className="rich-editor-image-fallback">
          <Paperclip className="h-4 w-4" />
          <span>{node.attrs.alt || t('notes.attachmentUnavailable')}</span>
        </div>
      ) : (
        <div className="rich-editor-image-loading" aria-label={t('common.loading')} />
      )}
    </NodeViewWrapper>
  )
}

const AttachmentImage = Image.extend({
  addNodeView() {
    return ReactNodeViewRenderer(AttachmentImageView)
  },
})

function ToolbarButton({
  label,
  active = false,
  disabled = false,
  onClick,
  children,
}: {
  label: string
  active?: boolean
  disabled?: boolean
  onClick: () => void
  children: ReactNode
}) {
  const keepSelection = (event: MouseEvent<HTMLButtonElement>) => event.preventDefault()
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      aria-pressed={active}
      disabled={disabled}
      onMouseDown={keepSelection}
      onClick={onClick}
      className={cn(
        'inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-[hsl(var(--muted))] transition-colors hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))] disabled:pointer-events-none disabled:opacity-35',
        active && 'bg-[hsl(var(--panel-2))] text-[hsl(var(--text))]',
      )}
    >
      {children}
    </button>
  )
}

export const RichEditor = forwardRef<RichEditorHandle, RichEditorProps>(function RichEditor({
  markdown,
  disabled = false,
  onChange,
  onAddImage,
  onAddAttachment,
  onOpenAttachment,
  toolbarRight,
}, ref) {
  const { t } = useTranslation()
  const extensions = useMemo(() => [
    StarterKit.configure({
      heading: { levels: [1, 2, 3] },
      link: false,
    }),
    Link.configure({
      autolink: false,
      openOnClick: false,
      protocols: ['colink-attachment'],
    }),
    AttachmentImage.configure({ inline: false, allowBase64: false }),
    Placeholder.configure({ placeholder: t('notes.markdownPlaceholder') }),
    TaskList,
    TaskItem.configure({ nested: true }),
    Markdown.configure({
      html: false,
      tightLists: true,
      transformPastedText: true,
      transformCopiedText: true,
    }),
  ], [t])

  const editor = useEditor({
    extensions,
    content: markdown,
    editable: !disabled,
    immediatelyRender: false,
    editorProps: {
      attributes: {
        class: 'rich-editor-content',
        spellcheck: 'true',
      },
      handleClick: (_view, _pos, event) => {
        const link = (event.target as HTMLElement).closest('a')
        const href = link?.getAttribute('href') ?? ''
        if (!ATTACHMENT_URI_PATTERN.test(href)) return false
        event.preventDefault()
        onOpenAttachment(href.slice(ATTACHMENT_URI_PREFIX.length))
        return true
      },
    },
    onUpdate: ({ editor: currentEditor }) => {
      onChange(getMarkdown(currentEditor))
    },
  }, [extensions])

  const toolbarState = useEditorState({
    editor,
    selector: ({ editor: currentEditor }) => currentEditor ? getToolbarState(currentEditor) : null,
  }) ?? (editor ? getToolbarState(editor) : null)

  useEffect(() => {
    editor?.setEditable(!disabled)
  }, [disabled, editor])

  useEffect(() => {
    if (!editor) return
    const currentMarkdown = getMarkdown(editor)
    if (currentMarkdown !== markdown) {
      editor.commands.setContent(markdown, { emitUpdate: false })
    }
  }, [editor, markdown])

  useImperativeHandle(ref, () => ({
    insertAttachment: (attachment, kind) => {
      if (!editor) return
      const uri = `${ATTACHMENT_URI_PREFIX}${attachment.id}`
      if (kind === 'image') {
        editor.chain().focus().setImage({ src: uri, alt: attachment.fileName }).run()
        return
      }
      editor.chain().focus().insertContent([
        {
          type: 'text',
          text: attachment.fileName,
          marks: [{ type: 'link', attrs: { href: uri } }],
        },
        { type: 'text', text: ' ' },
      ]).run()
    },
  }), [editor])

  if (!editor || !toolbarState) {
    return <div className="min-h-64 flex-1 animate-pulse bg-[hsl(var(--panel-2)/0.35)]" />
  }

  const actionDisabled = disabled || !editor.isEditable
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="rich-editor-toolbar" role="toolbar" aria-label={t('notes.formattingToolbar')}>
        <div className="flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto">
          <ToolbarButton label={t('notes.undo')} disabled={actionDisabled || !toolbarState.canUndo} onClick={() => editor.chain().focus().undo().run()}>
            <Undo2 className="h-4 w-4" />
          </ToolbarButton>
          <ToolbarButton label={t('notes.redo')} disabled={actionDisabled || !toolbarState.canRedo} onClick={() => editor.chain().focus().redo().run()}>
            <Redo2 className="h-4 w-4" />
          </ToolbarButton>
          <span className="rich-editor-separator" />
          {([1, 2, 3] as const).map((level) => {
            const HeadingIcon = level === 1 ? Heading1 : level === 2 ? Heading2 : Heading3
            return (
              <ToolbarButton
                key={level}
                label={t(`notes.heading${level}`)}
                active={toolbarState.headings[level - 1]}
                disabled={actionDisabled}
                onClick={() => editor.chain().focus().toggleHeading({ level }).run()}
              >
                <HeadingIcon className="h-4 w-4" />
              </ToolbarButton>
            )
          })}
          <span className="rich-editor-separator" />
          <ToolbarButton label={t('notes.bold')} active={toolbarState.bold} disabled={actionDisabled} onClick={() => editor.chain().focus().toggleBold().run()}>
            <Bold className="h-4 w-4" />
          </ToolbarButton>
          <ToolbarButton label={t('notes.italic')} active={toolbarState.italic} disabled={actionDisabled} onClick={() => editor.chain().focus().toggleItalic().run()}>
            <Italic className="h-4 w-4" />
          </ToolbarButton>
          <ToolbarButton label={t('notes.strike')} active={toolbarState.strike} disabled={actionDisabled} onClick={() => editor.chain().focus().toggleStrike().run()}>
            <Strikethrough className="h-4 w-4" />
          </ToolbarButton>
          <ToolbarButton label={t('notes.inlineCode')} active={toolbarState.inlineCode} disabled={actionDisabled} onClick={() => editor.chain().focus().toggleCode().run()}>
            <Code className="h-4 w-4" />
          </ToolbarButton>
          <span className="rich-editor-separator" />
          <ToolbarButton label={t('notes.bulletList')} active={toolbarState.bulletList} disabled={actionDisabled} onClick={() => editor.chain().focus().toggleBulletList().run()}>
            <List className="h-4 w-4" />
          </ToolbarButton>
          <ToolbarButton label={t('notes.orderedList')} active={toolbarState.orderedList} disabled={actionDisabled} onClick={() => editor.chain().focus().toggleOrderedList().run()}>
            <ListOrdered className="h-4 w-4" />
          </ToolbarButton>
          <ToolbarButton label={t('notes.todoList')} active={toolbarState.taskList} disabled={actionDisabled} onClick={() => editor.chain().focus().toggleTaskList().run()}>
            <ListChecks className="h-4 w-4" />
          </ToolbarButton>
          <ToolbarButton label={t('notes.quote')} active={toolbarState.blockquote} disabled={actionDisabled} onClick={() => editor.chain().focus().toggleBlockquote().run()}>
            <Quote className="h-4 w-4" />
          </ToolbarButton>
          <ToolbarButton label={t('notes.codeBlock')} active={toolbarState.codeBlock} disabled={actionDisabled} onClick={() => editor.chain().focus().toggleCodeBlock().run()}>
            <FileCode2 className="h-4 w-4" />
          </ToolbarButton>
          <span className="rich-editor-separator" />
          <ToolbarButton label={t('notes.addImage')} disabled={actionDisabled} onClick={onAddImage}>
            <ImageIcon className="h-4 w-4" />
          </ToolbarButton>
          <ToolbarButton label={t('notes.addAttachment')} disabled={actionDisabled} onClick={onAddAttachment}>
            <Paperclip className="h-4 w-4" />
          </ToolbarButton>
        </div>
        {toolbarRight && (
          <div className="ml-auto flex shrink-0 items-center gap-1 pl-2">
            <span className="rich-editor-separator" />
            {toolbarRight}
          </div>
        )}
      </div>
      <EditorContent editor={editor} className="min-h-0 flex-1 overflow-y-auto" />
    </div>
  )
})
