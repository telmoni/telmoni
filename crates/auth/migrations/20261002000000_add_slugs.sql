-- Migration: 20261002000000_add_slugs.sql
-- Add human-friendly URL slugs to organizations and projects with unique case-insensitive indexes.

-- 1. Organizations slug column
ALTER TABLE auth.organizations
    ADD COLUMN slug TEXT;

-- Backfill organization slug with external_id
UPDATE auth.organizations
    SET slug = external_id
    WHERE slug IS NULL;

-- Create case-insensitive unique index for active organizations
CREATE UNIQUE INDEX organizations_slug_lower_active_idx
    ON auth.organizations (lower(slug))
    WHERE status = 'active';

-- 2. Projects slug column
ALTER TABLE auth.projects
    ADD COLUMN slug TEXT;

-- Backfill project slug with external_id
UPDATE auth.projects
    SET slug = external_id
    WHERE slug IS NULL;

-- Create case-insensitive unique index for active projects per organization
CREATE UNIQUE INDEX projects_organization_slug_lower_active_idx
    ON auth.projects (organization_id, lower(slug))
    WHERE status = 'active';
