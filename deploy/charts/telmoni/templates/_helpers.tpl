{{/*
Expand the name of the chart.
*/}}
{{- define "telmoni.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Create a default fully qualified app name.
*/}}
{{- define "telmoni.fullname" -}}
{{- if .Values.fullnameOverride }}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}
{{- end }}

{{/*
What the migrate hook needs before it runs. A pre-install hook runs before
the release's ordinary objects exist, so its service account and the tier's
ConfigMap are hooks too, made first and kept (no hook-succeeded). Helm does
not delete hooks on uninstall.
*/}}
{{- define "telmoni.migrateHookPrereq" -}}
annotations:
  "helm.sh/hook": pre-install,pre-upgrade
  "helm.sh/hook-weight": "-10"
  "helm.sh/hook-delete-policy": before-hook-creation
{{- end }}

{{/*
Common labels
*/}}
{{- define "telmoni.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
app.kubernetes.io/part-of: telmoni
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}
