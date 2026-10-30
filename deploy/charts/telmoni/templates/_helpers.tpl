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
the release's ordinary objects exist, so its service account is a hook too,
made first and kept (no hook-succeeded). Helm does not delete hooks on
uninstall.
*/}}
{{- define "telmoni.migrateHookPrereq" -}}
annotations:
  "helm.sh/hook": pre-install,pre-upgrade
  "helm.sh/hook-weight": "-10"
  "helm.sh/hook-delete-policy": before-hook-creation
{{- end }}

{{/*
Every container's image of this repository's, at the one required tag.
*/}}
{{- define "telmoni.image" -}}
{{- $tag := required "global.imageTag: the full commit SHA this chart is checked out at" .root.Values.global.imageTag -}}
{{- with .root.Values.global.imageRegistry }}{{ . }}/{{ end }}{{ .name }}:{{ $tag }}
{{- end }}

{{/*
A store of `networkPolicies.stores` as a policy peer: its pods by label, in
the namespace its selector names, or in this one.
*/}}
{{- define "telmoni.storePeer" -}}
podSelector:
  matchLabels:
    {{- toYaml .podSelector | nindent 4 }}
{{- with .namespaceSelector }}
namespaceSelector:
  matchLabels:
    {{- toYaml . | nindent 4 }}
{{- end }}
{{- end }}

{{/*
With highAvailability, a Deployment's replicas spread across nodes, so one
node's loss leaves a replica; scheduled anyway on a single node.
*/}}
{{- define "telmoni.spread" -}}
topologySpreadConstraints:
  - maxSkew: 1
    topologyKey: kubernetes.io/hostname
    whenUnsatisfiable: ScheduleAnyway
    labelSelector:
      matchLabels:
        app.kubernetes.io/name: {{ . }}
{{- end }}

{{/*
Common labels
*/}}
{{- define "telmoni.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
app.kubernetes.io/part-of: telmoni
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end }}
