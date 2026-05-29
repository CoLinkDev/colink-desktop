import type { FormEvent, ReactNode } from 'react'
import { useEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { X } from 'lucide-react'
import { z } from 'zod'
import { useTranslation } from 'react-i18next'

import { readErrorMessage, useAppState } from '../hooks/use-app-state'
import { Button } from './ui/button'
import { Input } from './ui/input'

interface AuthDialogProps {
  open: boolean
  onClose: () => void
}

export function AuthDialog({ open, onClose }: AuthDialogProps) {
  const { t } = useTranslation()
  const { bootstrapError, login, register, saveSettings, session, settings } = useAppState()
  const [mode, setMode] = useState<'login' | 'register'>('login')
  const [serverUrl, setServerUrl] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [form, setForm] = useState({
    identifier: '',
    password: '',
  })
  const [registerForm, setRegisterForm] = useState({
    email: '',
    username: '',
    password: '',
    confirmPassword: '',
  })

  const loginSchema = useMemo(
    () =>
      z.object({
        identifier: z.string().min(1, t('auth.validation.identifier')),
        password: z.string().min(1, t('auth.validation.password')),
      }),
    [t],
  )

  const registerSchema = useMemo(
    () =>
      z
        .object({
          email: z.email(t('auth.validation.emailFormat')),
          username: z
            .string()
            .min(3, t('auth.validation.usernameLength'))
            .max(32, t('auth.validation.usernameMaxLength')),
          password: z.string().min(8, t('auth.validation.passwordLength')),
          confirmPassword: z.string().min(1, t('auth.validation.confirmPasswordRequired')),
        })
        .refine((value) => value.password === value.confirmPassword, {
          message: t('auth.validation.passwordMismatch'),
          path: ['confirmPassword'],
        }),
    [t],
  )

  useEffect(() => {
    if (settings.serverUrl) {
      setServerUrl(settings.serverUrl)
    }
  }, [settings.serverUrl])

  useEffect(() => {
    if (session && open) {
      onClose()
    }
  }, [onClose, open, session])

  useEffect(() => {
    if (open) {
      setError(null)
      setSubmitting(false)
    }
  }, [open])

  if (!open) {
    return null
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setError(null)

    if (!serverUrl.trim()) {
      setError(t('auth.serverRequired'))
      return
    }

    setSubmitting(true)
    try {
      await saveSettings({
        ...settings,
        serverUrl: serverUrl.trim(),
      })

      if (mode === 'login') {
        const parsed = loginSchema.safeParse(form)
        if (!parsed.success) {
          setError(parsed.error.issues[0]?.message ?? t('auth.formIncomplete'))
          return
        }
        await login(parsed.data)
      } else {
        const parsed = registerSchema.safeParse(registerForm)
        if (!parsed.success) {
          setError(parsed.error.issues[0]?.message ?? t('auth.formIncomplete'))
          return
        }
        await register({
          email: parsed.data.email,
          username: parsed.data.username,
          password: parsed.data.password,
        })
      }

      onClose()
    } catch (requestError) {
      setError(readErrorMessage(requestError))
    } finally {
      setSubmitting(false)
    }
  }

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm animate-fade-in">
      <div className="w-full max-w-sm rounded-xl border bg-[hsl(var(--panel))] p-6 shadow-xl animate-scale-in">
        <div className="flex items-start justify-between gap-4">
          <div>
            <div className="text-[16px] font-semibold text-[hsl(var(--text))]">{t('auth.title')}</div>
            <p className="mt-1 text-[12px] text-[hsl(var(--muted))]">{t('auth.subtitle')}</p>
          </div>
          <button
            className="flex h-8 w-8 items-center justify-center rounded-lg text-[hsl(var(--muted))] hover:bg-[hsl(var(--panel-2))] hover:text-[hsl(var(--text))]"
            disabled={submitting}
            onClick={onClose}
            title={t('common.close')}
            type="button"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="mt-5 grid grid-cols-2 rounded-lg border bg-[hsl(var(--panel-2))] p-1">
          <button
            className={mode === 'login'
              ? 'rounded-md bg-[hsl(var(--panel))] px-3 py-1.5 text-[12px] font-medium text-[hsl(var(--text))] shadow-sm'
              : 'rounded-md px-3 py-1.5 text-[12px] font-medium text-[hsl(var(--muted))] hover:text-[hsl(var(--text))]'}
            disabled={submitting}
            onClick={() => {
              setMode('login')
              setError(null)
            }}
            type="button"
          >
            {t('auth.login')}
          </button>
          <button
            className={mode === 'register'
              ? 'rounded-md bg-[hsl(var(--panel))] px-3 py-1.5 text-[12px] font-medium text-[hsl(var(--text))] shadow-sm'
              : 'rounded-md px-3 py-1.5 text-[12px] font-medium text-[hsl(var(--muted))] hover:text-[hsl(var(--text))]'}
            disabled={submitting}
            onClick={() => {
              setMode('register')
              setError(null)
            }}
            type="button"
          >
            {t('auth.register')}
          </button>
        </div>

        {(bootstrapError || error) && (
          <div className="mt-5 rounded-lg bg-[hsl(var(--danger)/0.08)] px-4 py-2.5 text-[13px] text-[hsl(var(--danger))]">
            {error ?? bootstrapError}
          </div>
        )}

        <form className="mt-5 space-y-4" onSubmit={handleSubmit}>
          <Field label={t('auth.server')}>
            <Input
              onChange={(event) => setServerUrl(event.target.value)}
              placeholder="http://127.0.0.1:8080"
              value={serverUrl}
            />
          </Field>

          {mode === 'login' ? (
            <>
              <Field label={t('auth.account')}>
                <Input
                  autoComplete="username"
                  onChange={(event) =>
                    setForm((current) => ({
                      ...current,
                      identifier: event.target.value,
                    }))
                  }
                  placeholder={t('auth.accountPlaceholder')}
                  value={form.identifier}
                />
              </Field>

              <Field label={t('auth.password')}>
                <Input
                  autoComplete="current-password"
                  onChange={(event) =>
                    setForm((current) => ({
                      ...current,
                      password: event.target.value,
                    }))
                  }
                  placeholder={t('auth.passwordPlaceholder')}
                  type="password"
                  value={form.password}
                />
              </Field>
            </>
          ) : (
            <>
              <Field label={t('auth.email')}>
                <Input
                  autoComplete="email"
                  onChange={(event) =>
                    setRegisterForm((current) => ({
                      ...current,
                      email: event.target.value,
                    }))
                  }
                  placeholder={t('auth.emailPlaceholder')}
                  type="email"
                  value={registerForm.email}
                />
              </Field>

              <Field label={t('auth.username')}>
                <Input
                  autoComplete="username"
                  onChange={(event) =>
                    setRegisterForm((current) => ({
                      ...current,
                      username: event.target.value,
                    }))
                  }
                  placeholder={t('auth.usernamePlaceholder')}
                  value={registerForm.username}
                />
              </Field>

              <Field label={t('auth.password')}>
                <Input
                  autoComplete="new-password"
                  onChange={(event) =>
                    setRegisterForm((current) => ({
                      ...current,
                      password: event.target.value,
                    }))
                  }
                  placeholder={t('auth.passwordPlaceholder')}
                  type="password"
                  value={registerForm.password}
                />
              </Field>

              <Field label={t('auth.confirmPassword')}>
                <Input
                  autoComplete="new-password"
                  onChange={(event) =>
                    setRegisterForm((current) => ({
                      ...current,
                      confirmPassword: event.target.value,
                    }))
                  }
                  placeholder={t('auth.confirmPasswordPlaceholder')}
                  type="password"
                  value={registerForm.confirmPassword}
                />
              </Field>
            </>
          )}

          <div className="flex justify-end gap-2 pt-2">
            <Button disabled={submitting} onClick={onClose} type="button" variant="secondary">
              {t('common.cancel')}
            </Button>
            <Button disabled={submitting} type="submit" variant="primary">
              {submitting
                ? mode === 'login'
                  ? t('auth.loggingIn')
                  : t('auth.registering')
                : mode === 'login'
                  ? t('auth.login')
                  : t('auth.register')}
            </Button>
          </div>
        </form>
      </div>
    </div>,
    document.body,
  )
}

interface FieldProps {
  label: string
  children: ReactNode
}

function Field({ label, children }: FieldProps) {
  return (
    <label className="block space-y-1.5">
      <span className="text-[12px] font-medium text-[hsl(var(--text-secondary))]">{label}</span>
      {children}
    </label>
  )
}
