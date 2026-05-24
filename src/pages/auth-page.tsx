import type { FormEvent, ReactNode } from 'react'
import { useState, useEffect } from 'react'
import { Navigate, useNavigate } from 'react-router-dom'
import { z } from 'zod'

import { Button } from '../components/ui/button'
import { Input } from '../components/ui/input'
import { readErrorMessage, useAppState } from '../hooks/use-app-state'

const loginSchema = z.object({
  identifier: z.string().min(1, '请输入邮箱或用户名'),
  password: z.string().min(1, '请输入密码'),
})

const registerSchema = z
  .object({
    email: z.string().email('邮箱格式不对'),
    username: z
      .string()
      .min(3, '用户名至少 3 位')
      .max(32, '用户名不能超过 32 位'),
    password: z.string().min(8, '密码至少 8 位'),
    confirmPassword: z.string().min(8, '请再次输入密码'),
  })
  .refine((value) => value.password === value.confirmPassword, {
    message: '两次密码不一致',
    path: ['confirmPassword'],
  })

export function AuthPage() {
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
      setError('请输入服务器地址')
      return
    }

    const parsed = loginSchema.safeParse(loginForm)

    if (!parsed.success) {
      setError(parsed.error.issues[0]?.message ?? '请输入完整信息')
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
      setError(parsed.error.issues[0]?.message ?? '请输入完整信息')
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
          <div className="text-[22px] font-semibold tracking-tight">CoLink</div>
          <p className="mt-2 text-[13px] text-[hsl(var(--muted))]">
            连接账户以同步设备
          </p>
        </div>

        {(bootstrapError || error) && (
          <div className="mt-6 rounded-lg bg-[hsl(var(--danger)/0.08)] px-4 py-2.5 text-[13px] text-[hsl(var(--danger))]">
            {error ?? bootstrapError}
          </div>
        )}

        {mode === 'login' ? (
          <form className="mt-8 space-y-4" onSubmit={handleLogin}>
            <Field label="服务器">
              <Input
                onChange={(event) => setServerUrl(event.target.value)}
                placeholder="http://127.0.0.1:8080"
                value={serverUrl}
              />
            </Field>

            <Field label="账户">
              <Input
                autoComplete="username"
                onChange={(event) =>
                  setLoginForm((current) => ({
                    ...current,
                    identifier: event.target.value,
                  }))
                }
                placeholder="邮箱或用户名"
                value={loginForm.identifier}
              />
            </Field>

            <Field label="密码">
              <Input
                autoComplete="current-password"
                onChange={(event) =>
                  setLoginForm((current) => ({
                    ...current,
                    password: event.target.value,
                  }))
                }
                placeholder="请输入密码"
                type="password"
                value={loginForm.password}
              />
            </Field>

            <div className="pt-2">
              <Button className="w-full" disabled={submitting} type="submit">
                {submitting ? '连接中…' : '登录'}
              </Button>
            </div>
          </form>
        ) : (
          <form className="mt-8 space-y-4" onSubmit={handleRegister}>
            <Field label="邮箱">
              <Input
                autoComplete="email"
                onChange={(event) =>
                  setRegisterForm((current) => ({
                    ...current,
                    email: event.target.value,
                  }))
                }
                placeholder="user@example.com"
                value={registerForm.email}
              />
            </Field>

            <Field label="用户名">
              <Input
                autoComplete="username"
                onChange={(event) =>
                  setRegisterForm((current) => ({
                    ...current,
                    username: event.target.value,
                  }))
                }
                placeholder="brook.user"
                value={registerForm.username}
              />
            </Field>

            <Field label="密码">
              <Input
                autoComplete="new-password"
                onChange={(event) =>
                  setRegisterForm((current) => ({
                    ...current,
                    password: event.target.value,
                  }))
                }
                placeholder="至少 8 位"
                type="password"
                value={registerForm.password}
              />
            </Field>

            <Field label="确认密码">
              <Input
                autoComplete="new-password"
                onChange={(event) =>
                  setRegisterForm((current) => ({
                    ...current,
                    confirmPassword: event.target.value,
                  }))
                }
                placeholder="再次输入密码"
                type="password"
                value={registerForm.confirmPassword}
              />
            </Field>

            <div className="pt-2">
              <Button className="w-full" disabled={submitting} type="submit">
                {submitting ? '正在创建…' : '注册'}
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
