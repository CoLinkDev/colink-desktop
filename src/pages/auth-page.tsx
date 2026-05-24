import type { FormEvent, ReactNode } from 'react'
import { useState, useEffect, useMemo } from 'react'
import { Navigate, useNavigate } from 'react-router-dom'
import { z } from 'zod'
import { useTranslation } from 'react-i18next'

import { Button } from '../components/ui/button'
import { Input } from '../components/ui/input'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'

export function AuthPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { session, status, login, register, bootstrapError, settings, saveSettings } = useAppState()
  const [mode] = useState<'login' | 'register'>('login')
  const [serverUrl, setServerUrl] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [loginForm, setLoginForm] = useState({
    identifier: '',
    password: '',
  })
  const [registerForm, setRegisterForm] = useState({
    email: '',
    username: '',
    password: '',
    confirmPassword: '',
  })

  const loginSchema = useMemo(() => z.object({
    identifier: z.string().min(1, t('auth.validation.identifier')),
    password: z.string().min(1, t('auth.validation.password')),
  }), [t])

  const registerSchema = useMemo(() => z
    .object({
      email: z.string().email(t('auth.validation.emailFormat')),
      username: z
        .string()
        .min(3, t('auth.validation.usernameLength'))
        .max(32, t('auth.validation.usernameMaxLength')),
      password: z.string().min(8, t('auth.validation.passwordLength')),
      confirmPassword: z.string().min(8, t('auth.validation.confirmPasswordRequired')),
    })
    .refine((value) => value.password === value.confirmPassword, {
      message: t('auth.validation.passwordMismatch'),
      path: ['confirmPassword'],
    }), [t])

  useEffect(() => {
    if (settings.serverUrl) {
      setServerUrl(settings.serverUrl)
    }
  }, [settings.serverUrl])

  if (status === 'booting') {
    return null
  }

  if (session) {
    return <Navigate replace to="/devices" />
  }

  async function handleLogin(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setError(null)

    if (!serverUrl.trim()) {
      setError(t('auth.serverRequired'))
      return
    }

    const parsed = loginSchema.safeParse(loginForm)

    if (!parsed.success) {
      setError(parsed.error.issues[0]?.message ?? t('auth.formIncomplete'))
      return
    }

    setSubmitting(true)

    try {
      await saveSettings({
        ...settings,
        serverUrl: serverUrl.trim(),
      })
      await login(parsed.data)
      navigate('/devices')
    } catch (requestError) {
      setError(readErrorMessage(requestError))
    } finally {
      setSubmitting(false)
    }
  }

  async function handleRegister(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setError(null)

    const parsed = registerSchema.safeParse(registerForm)

    if (!parsed.success) {
      setError(parsed.error.issues[0]?.message ?? t('auth.formIncomplete'))
      return
    }

    setSubmitting(true)

    try {
      await register({
        email: parsed.data.email,
        username: parsed.data.username,
        password: parsed.data.password,
      })
      navigate('/devices')
    } catch (requestError) {
      setError(readErrorMessage(requestError))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-[hsl(var(--background))] px-6">
      <div className="w-full max-w-sm animate-slide-up">
        <div className="text-center">
          <div className="text-[22px] font-semibold tracking-tight">{t('auth.title')}</div>
          <p className="mt-2 text-[13px] text-[hsl(var(--muted))]">
            {t('auth.subtitle')}
          </p>
        </div>

        {(bootstrapError || error) && (
          <div className="mt-6 rounded-lg bg-[hsl(var(--danger)/0.08)] px-4 py-2.5 text-[13px] text-[hsl(var(--danger))]">
            {error ?? bootstrapError}
          </div>
        )}

        {mode === 'login' ? (
          <form className="mt-8 space-y-4" onSubmit={handleLogin}>
            <Field label={t('auth.server')}>
              <Input
                onChange={(event) => setServerUrl(event.target.value)}
                placeholder="http://127.0.0.1:8080"
                value={serverUrl}
              />
            </Field>

            <Field label={t('auth.account')}>
              <Input
                autoComplete="username"
                onChange={(event) =>
                  setLoginForm((current) => ({
                    ...current,
                    identifier: event.target.value,
                  }))
                }
                placeholder={t('auth.accountPlaceholder')}
                value={loginForm.identifier}
              />
            </Field>

            <Field label={t('auth.password')}>
              <Input
                autoComplete="current-password"
                onChange={(event) =>
                  setLoginForm((current) => ({
                    ...current,
                    password: event.target.value,
                  }))
                }
                placeholder={t('auth.passwordPlaceholder')}
                type="password"
                value={loginForm.password}
              />
            </Field>

            <div className="pt-2">
              <Button className="w-full" disabled={submitting} type="submit">
                {submitting ? t('auth.loggingIn') : t('auth.login')}
              </Button>
            </div>
          </form>
        ) : (
          <form className="mt-8 space-y-4" onSubmit={handleRegister}>
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
                placeholder={t('auth.validation.passwordLength')}
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

            <div className="pt-2">
              <Button className="w-full" disabled={submitting} type="submit">
                {submitting ? t('auth.registering') : t('auth.register')}
              </Button>
            </div>
          </form>
        )}
      </div>
    </div>
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
